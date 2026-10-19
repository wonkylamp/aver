import type { Request, Response } from "./router.ts";

// Adds CORS headers when the request's Origin is allowed. "*" in the list
// allows any origin, but the header still echoes the caller's origin so that
// credentialed requests keep working.
export function withCors(allowedOrigins: string[], req: Request, res: Response): Response {
  const origin = req.headers["origin"];
  if (origin === undefined || origin === "") return res;
  if (!allowedOrigins.includes("*") && !allowedOrigins.includes(origin)) return res;
  return {
    ...res,
    headers: { ...res.headers, "access-control-allow-origin": origin, vary: "Origin" },
  };
}
