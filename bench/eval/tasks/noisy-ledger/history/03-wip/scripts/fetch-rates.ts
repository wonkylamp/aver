// Downloads the ECB's 2025 reference rates into fixtures/rates-2025.json.
import { writeFileSync } from "node:fs";
import { RATES_FILE } from "../src/fx.ts";

const URL = "https://data-api.ecb.europa.eu/service/data/EXR/D..EUR.SP00.A?startPeriod=2025-01-01&endPeriod=2025-12-31&format=csvdata";

const res = await fetch(URL);
if (!res.ok) throw new Error(`ECB answered ${res.status}`);
const table: Record<string, Record<string, number>> = {};
const [header, ...lines] = (await res.text()).trim().split("\n");
const cols = header!.split(",");
const [cur, day, value] = ["CURRENCY", "TIME_PERIOD", "OBS_VALUE"].map((c) => cols.indexOf(c));
for (const line of lines) {
  const f = line.split(",");
  (table[f[day!]!] ??= {})[f[cur!]!] = Number(f[value!]);
}
writeFileSync(RATES_FILE, JSON.stringify(table));
console.log(`wrote ${Object.keys(table).length} days to ${RATES_FILE}`);
