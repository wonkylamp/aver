import { test } from "node:test";
import assert from "node:assert/strict";
import { Metrics } from "../src/metrics/registry.ts";

test("counters add up per label set", () => {
  const m = new Metrics("beacon_");
  m.inc("requests_total", { code: "2xx" });
  m.inc("requests_total", { code: "2xx" });
  m.inc("requests_total", { code: "5xx" });
  assert.equal(m.value("requests_total", { code: "2xx" }), 2);
  assert.equal(m.value("requests_total", { code: "5xx" }), 1);
});

test("render is sorted Prometheus text", () => {
  const m = new Metrics("b_");
  m.set("queue_depth", 4);
  m.inc("hits", { route: "/x", code: "2xx" });
  assert.equal(
    m.render(),
    ['# TYPE b_hits counter', 'b_hits{code="2xx",route="/x"} 1', "# TYPE b_queue_depth gauge", "b_queue_depth 4", ""].join("\n"),
  );
});

test("label values are escaped", () => {
  const m = new Metrics("");
  m.inc("odd", { v: 'a"b' });
  assert.match(m.render(), /odd\{v="a\\"b"\} 1/);
});

test("a counter cannot decrease or turn into a gauge", () => {
  const m = new Metrics("x_");
  assert.throws(() => m.inc("c", {}, -1), /cannot decrease/);
  m.inc("c");
  assert.throws(() => m.set("c", 1), /is a counter/);
});

test("names are checked with the prefix applied", () => {
  assert.throws(() => new Metrics("bad-").inc("c"), /invalid metric name/);
});
