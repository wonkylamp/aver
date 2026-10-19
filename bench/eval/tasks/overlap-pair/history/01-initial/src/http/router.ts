export interface Request {
  method: string;
  path: string;
  headers: Record<string, string>;
}

export interface Response {
  status: number;
  headers: Record<string, string>;
  body: string;
}

export type Params = Record<string, string>;

export type Handler = (req: Request, params: Params) => Response | Promise<Response>;

interface Route {
  method: string;
  parts: string[];
  handler: Handler;
}

export class Router {
  #routes: Route[] = [];

  add(method: string, pattern: string, handler: Handler): this {
    this.#routes.push({ method: method.toUpperCase(), parts: split(pattern), handler });
    return this;
  }

  async handle(req: Request): Promise<Response> {
    const parts = split(req.path);
    let pathMatched = false;
    for (const route of this.#routes) {
      const params = match(route.parts, parts);
      if (params === null) continue;
      pathMatched = true;
      if (route.method === req.method.toUpperCase()) return route.handler(req, params);
    }
    return pathMatched ? text(405, "method not allowed") : text(404, "not found");
  }
}

export function json(status: number, value: unknown): Response {
  return { status, headers: { "content-type": "application/json" }, body: JSON.stringify(value) };
}

export function text(status: number, body: string): Response {
  return { status, headers: { "content-type": "text/plain; charset=utf-8" }, body };
}

function split(path: string): string[] {
  const withoutQuery = path.split("?", 1)[0];
  return withoutQuery.split("/").filter((part) => part !== "");
}

function match(pattern: string[], parts: string[]): Params | null {
  if (pattern.length !== parts.length) return null;
  const params: Params = {};
  for (let i = 0; i < pattern.length; i++) {
    const want = pattern[i];
    if (want.startsWith(":")) {
      try {
        params[want.slice(1)] = decodeURIComponent(parts[i]);
      } catch {
        return null;
      }
    } else if (want !== parts[i]) {
      return null;
    }
  }
  return params;
}
