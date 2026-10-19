// The "What agents asked for" section of results.md: the answers to the
// last feedback question ("What one change would raise your rating the
// most?"), grouped into recurring requests by keywords, most common first.

type Answered = { arm: "A" | "B"; feedback: { answer: string } | { error: string } | { rating: number | null; answer: string } | null };

/** A request and the words that mark it; an answer can make several. */
export const REQUESTS: Array<{ request: string; re: RegExp }> = [
  { request: "A recorded baseline: which failures already existed before my change", re: /baseline|pre-?existing|already (fail|red|broken|existed)|known(-| )fail|before[- ]and[- ]after|before\/after|new failures|my failures apart/i },
  { request: "Flaky-test detection (rerun a test, report its pass rate)", re: /flak|re-?run .{0,40}times|pass rate/i },
  { request: "Visibility into other tasks touching the same code", re: /other (active )?(task|branch|worktree|agent)|overlap|sibling|parallel work|would merge|conflict/i },
  { request: "The verdict repeated on the last line of the output", re: /last line|\btail\b|end of (the )?(long )?output/i },
  { request: "A type-check in the test command", re: /type-?check|\btsc\b|typescript as a dev/i },
  { request: "CI parity: run CI's checks locally, see CI logs", re: /\bci\b|pipeline/i },
  { request: "Fewer permission prompts for read-only commands", re: /permission|approval|without (asking|prompt)|allowlist/i },
  { request: "Clearer done criteria (e.g. failing tests become criteria)", re: /no criteria|criteria (are|is) missing|tests? (become|as|the) criteria|done condition|acceptance criteri|did the job|definition of done/i },
  { request: "Stronger proofs (a load failure is not a proof)", re: /\bprov(e|en|ing)\b|proof|import failure|missing module/i },
  { request: "More verifies, or the remaining count shown", re: /verifies (used|left)|remaining verif|verify budget|wasted (one|a) verif/i },
];

/** The answer to the last question, or the last paragraph when the numbering is lost. */
export function lastAnswer(answer: string): string {
  const m = /(?:^|\n)[\s#*>]*4\s*[.):]/.exec(answer);
  const text = (m ? answer.slice(m.index + m[0].length) : (answer.trim().split(/\n\s*\n/).at(-1) ?? "")).trim();
  // Drop a restated question ("**The one change that would raise my rating most**").
  const nl = text.indexOf("\n");
  const head = nl < 0 ? "" : text.slice(0, nl);
  return head.length < 90 && /change|help/i.test(head) && /most|rating/i.test(head) ? text.slice(nl + 1).trim() : text;
}

/** The first sentence of `text` that `re` matches (else the first sentence), for a table cell. */
function example(text: string, re?: RegExp): string {
  const flat = text.replace(/\s+/g, " ").replace(/\*\*/g, "").trim();
  const sentences = flat.split(/(?<=[.!?])\s+(?=[A-Z`"(])/);
  const s = (re && sentences.find((x) => re.test(x))) || sentences[0] || flat;
  return (s.length > 160 ? `${s.slice(0, 157)}...` : s).replace(/\|/g, "\\|");
}

/** Markdown lines of the section, or none when no agent answered. */
export function askedFor(attempts: Answered[]): string[] {
  const answers = attempts.flatMap((a) => (a.feedback && "answer" in a.feedback && a.feedback.answer.trim() ? [{ arm: a.arm, text: lastAnswer(a.feedback.answer) }] : []));
  if (answers.length === 0) return [];
  const lines = [
    "## What agents asked for",
    "",
    `Each agent's answer to "What one change would raise your rating the most?", matched to recurring requests by keywords (\`bench/eval/lib/asked.ts\`); an answer can make several requests. ${answers.filter((a) => a.arm === "A").length} answers from arm A, ${answers.filter((a) => a.arm === "B").length} from arm B.`,
    "",
    "| Arm | Request | Agents | Example |",
    "|---|---|---|---|",
  ];
  for (const arm of ["A", "B"] as const) {
    const mine = answers.filter((a) => a.arm === arm);
    const rows = REQUESTS.map((r) => ({ r, hits: mine.filter((a) => r.re.test(a.text)) }))
      .filter((x) => x.hits.length > 0)
      .sort((x, y) => y.hits.length - x.hits.length)
      .slice(0, 5);
    for (const { r, hits } of rows) lines.push(`| ${arm} | ${r.request} | ${hits.length}/${mine.length} | ${example(hits[0]!.text, r.re)} |`);
    const other = mine.filter((a) => !REQUESTS.some((r) => r.re.test(a.text)));
    if (other.length) lines.push(`| ${arm} | (none of the above) | ${other.length}/${mine.length} | ${example(other[0]!.text)} |`);
  }
  return lines;
}
