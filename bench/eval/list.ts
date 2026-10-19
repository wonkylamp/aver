// Prints each scenario's tasks as the agents get them: the statement, the
// criteria, and what the scenario measures.
//
//   node bench/eval/list.ts [scenario ...]
import { loadScenario, scenarioNames } from "./lib/task.ts";

const names = process.argv.slice(2);
for (const name of names.length > 0 ? names : scenarioNames()) {
  const s = loadScenario(name);
  console.log(`## ${name}: ${s.title}\n\n${s.measures}\n`);
  console.log(`History: ${s.history.map((h) => h.message).join(" / ")}`);
  console.log(`npm test failing at base: ${s.visible_failing}\n`);
  for (const t of s.tasks) {
    console.log(`### ${t.id}\n\n${t.statement.trim()}\n`);
    for (const c of t.criteria) console.log(`- criterion ${c}`);
    console.log(`fakes: ${t.fakes.map((f) => `${f.behavior} ${JSON.stringify(f.expect ?? {})}`).join("; ")}\n`);
  }
}
