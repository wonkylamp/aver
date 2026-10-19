// A storage report like the nightly job prints.
import { formatBytes, groupBy, ordinal, titleCase } from "../src/index.ts";

const files = [
  { team: "growth", bytes: 3_500_000 },
  { team: "checkout", bytes: 12_400_000_000 },
  { team: "growth", bytes: 900 },
  { team: "data platform", bytes: 640_000_000 },
];

const byTeam = [...groupBy(files, (f) => f.team)].map(([team, fs]) => ({ team, bytes: fs.reduce((n, f) => n + f.bytes, 0) }));
byTeam.sort((a, b) => b.bytes - a.bytes);
byTeam.forEach((t, i) => console.log(`${ordinal(i + 1).padEnd(4)} ${titleCase(t.team).padEnd(16)} ${formatBytes(t.bytes)}`));
