// bench/replay/replay.ts takes its manifest from --manifest or
// AVER_REPLAY_MANIFEST and names a missing one, since the in-repo default is
// private and may be absent. `--limit 0` stops before any pull request runs.
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const REPLAY = resolve(import.meta.dirname, "../../replay/replay.ts");

function replay(dir: string, args: string[], env: Record<string, string> = {}) {
  return spawnSync(process.execPath, [REPLAY, "--results", join(dir, "results.jsonl"), "--limit", "0", ...args], {
    encoding: "utf8",
    env: { ...process.env, AVER_REPLAY_WORK: join(dir, "work"), ...env },
  });
}

test("replay names a missing --manifest instead of failing to read it", () => {
  const dir = mkdtempSync(join(tmpdir(), "replay-manifest-"));
  try {
    const missing = join(dir, "absent.json");
    const r = replay(dir, ["--manifest", missing]);
    assert.notEqual(r.status, 0);
    assert.ok(r.stderr.includes(`manifest not found: ${missing} (from --manifest)`), r.stderr);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("replay reads the manifest path from AVER_REPLAY_MANIFEST", () => {
  const dir = mkdtempSync(join(tmpdir(), "replay-manifest-"));
  try {
    const missing = join(dir, "absent.json");
    const r = replay(dir, [], { AVER_REPLAY_MANIFEST: missing });
    assert.notEqual(r.status, 0);
    assert.ok(r.stderr.includes(`manifest not found: ${missing} (from AVER_REPLAY_MANIFEST)`), r.stderr);

    const manifest = join(dir, "manifest.json");
    writeFileSync(manifest, JSON.stringify({ prs: [] }));
    const ok = replay(dir, [], { AVER_REPLAY_MANIFEST: manifest });
    assert.equal(ok.status, 0, ok.stderr);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
