import { test } from "node:test";
import assert from "node:assert/strict";
import { HealthRegistry } from "../src/health/checks.ts";

test("all enabled checks passing is ok", async () => {
  const health = new HealthRegistry();
  health.register("store", () => {});
  health.register("queue", async () => {});
  assert.deepEqual(await health.run(["store", "queue"]), {
    status: "ok",
    checks: [
      { name: "store", ok: true },
      { name: "queue", ok: true },
    ],
  });
});

test("a throwing check degrades the report with its message", async () => {
  const health = new HealthRegistry();
  health.register("store", () => {
    throw new Error("disk full");
  });
  const report = await health.run(["store"]);
  assert.equal(report.status, "degraded");
  assert.deepEqual(report.checks[0], { name: "store", ok: false, detail: "disk full" });
});

test("an enabled check nobody registered fails", async () => {
  const report = await new HealthRegistry().run(["upstream"]);
  assert.deepEqual(report.checks[0], { name: "upstream", ok: false, detail: "not registered" });
});

test("checks that are not enabled do not run", async () => {
  const health = new HealthRegistry();
  let ran = false;
  health.register("slow", () => {
    ran = true;
  });
  await health.run([]);
  assert.equal(ran, false);
});

test("registering a name twice is an error", () => {
  const health = new HealthRegistry();
  health.register("store", () => {});
  assert.throws(() => health.register("store", () => {}), /already registered/);
});
