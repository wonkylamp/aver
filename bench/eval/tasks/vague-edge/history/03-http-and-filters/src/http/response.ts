export interface HttpResponse {
  status: number;
  headers: Record<string, string>;
  body: unknown;
}

export function json(status: number, body: unknown, headers: Record<string, string> = {}): HttpResponse {
  return { status, headers: { 'content-type': 'application/json; charset=utf-8', ...headers }, body };
}

export function errorResponse(status: number, error: string, extra: Record<string, unknown> = {}): HttpResponse {
  return json(status, { error, ...extra });
}
