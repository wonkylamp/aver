import { test } from "node:test";
import assert from "node:assert/strict";
import { createLogger, parseLevel } from "../src/logger.ts";

const fixed = () => new Date("2026-01-02T03:04:05.000Z");

function capture() {
  const lines: string[] = [];
  return { lines, sink: { write: (line: string) => lines.push(line) } };
}

test("messages below the level are dropped", () => {
  const { lines, sink } = capture();
  const log = createLogger({ level: "warn", json: true, sink, now: fixed });
  log.info("hidden");
  log.warn("shown");
  log.error("shown too");
  assert.equal(lines.length, 2);
});

test("json lines carry time, level, message and fields", () => {
  const { lines, sink } = capture();
  createLogger({ level: "debug", json: true, sink, now: fixed }).info("hello", { user: "u1" });
  assert.deepEqual(JSON.parse(lines[0]), {
    time: "2026-01-02T03:04:05.000Z",
    level: "info",
    msg: "hello",
    user: "u1",
  });
});

test("plain text quotes values that need it", () => {
  const { lines, sink } = capture();
  createLogger({ level: "info", json: false, sink, now: fixed }).warn("slow", { path: "/a b", ms: 12 });
  assert.equal(lines[0], '2026-01-02T03:04:05.000Z WARN  slow path="/a b" ms=12');
});

test("child loggers keep their bound fields", () => {
  const { lines, sink } = capture();
  const log = createLogger({ level: "info", json: true, sink, now: fixed }).child({ component: "health" });
  log.info("tick", { n: 1 });
  assert.equal(JSON.parse(lines[0]).component, "health");
  assert.equal(JSON.parse(lines[0]).n, 1);
});

test("levels parse case-insensitively and reject unknown names", () => {
  assert.equal(parseLevel(" WARN "), "warn");
  assert.throws(() => parseLevel("verbose"), /unknown log level/);
});
