import type { AppConfig } from "./config/schema.ts";
import { createLogger } from "./logger.ts";
import type { LogSink, Logger } from "./logger.ts";
import { Router, json, text } from "./http/router.ts";
import type { Request, Response } from "./http/router.ts";
import { withCors } from "./http/cors.ts";
import { HealthRegistry } from "./health/checks.ts";
import type { Check } from "./health/checks.ts";
import { Metrics } from "./metrics/registry.ts";
import { createFlags } from "./flags/flags.ts";
import type { Flags } from "./flags/flags.ts";

export interface AppDeps {
  sink: LogSink;
  now?: () => Date;
  checks?: Record<string, Check>;
}

export interface App {
  handle(req: Request): Promise<Response>;
  logger: Logger;
  metrics: Metrics;
  flags: Flags;
}

export function createApp(config: AppConfig, deps: AppDeps): App {
  const logger = createLogger({
    level: config.log.level,
    json: config.log.json,
    color: config.log.color,
    sink: deps.sink,
    now: deps.now,
  });
  const metrics = new Metrics(config.metrics.prefix);
  const flags = createFlags(config.features.enabled);
  const health = new HealthRegistry();
  for (const [name, check] of Object.entries(deps.checks ?? {})) health.register(name, check);

  const router = new Router();
  router.add("GET", "/healthz", async () => {
    const report = await health.run(config.health.checks);
    return json(report.status === "ok" ? 200 : 503, report);
  });
  if (config.metrics.enabled) {
    router.add("GET", "/metrics", () => text(200, metrics.render()));
  }
  router.add("GET", "/flags/:name", (_req, params) =>
    json(200, { name: params.name, enabled: flags.isEnabled(params.name) }),
  );

  return {
    async handle(req) {
      const res = withCors(config.server.allowedOrigins, req, await router.handle(req));
      metrics.inc("requests_total", { code: `${Math.floor(res.status / 100)}xx` });
      logger.info("request", { method: req.method, path: req.path, status: res.status });
      return res;
    },
    logger,
    metrics,
    flags,
  };
}
