// Today's item-20 push under the plain SDK, for comparison with _session/steering.
import { query } from "@anthropic-ai/claude-agent-sdk";
const msgs: any[] = [];
let push: (m: any) => void = () => {}; let close = () => {};
async function* input() {
  yield { type: "user", message: { role: "user", content: "Run `bun -e \"await Bun.sleep(10000)\"` with Bash in the foreground, then say 'first done'." }, parent_tool_use_id: null, session_id: "" };
  const q: any[] = []; let wake: any = null, done = false;
  push = (m) => { q.push(m); wake?.(); }; close = () => { done = true; wake?.(); };
  while (!done) { if (q.length) yield q.shift(); else await new Promise((r) => (wake = r)); }
}
const t = performance.now(); let pushed = false, results = 0;
const log: string[] = [];
for await (const m of query({ prompt: input(), options: { cwd: "/tmp/sb-acp/repo", permissionMode: "auto" } }) as any) {
  const at = Math.round(performance.now() - t);
  if (m.type === "assistant") for (const c of m.message.content) {
    if (c.type === "tool_use") { log.push(`${at} tool_use ${c.name} ${JSON.stringify(c.input).slice(0, 80)}`);
      if (!pushed && c.name === "Bash") { pushed = true; setTimeout(() => { log.push(`${Math.round(performance.now() - t)} PUSH`); push({ type: "user", message: { role: "user", content: "Also include the word KIWI in your final reply." }, parent_tool_use_id: null, session_id: "" }); }, 4000); } }
    if (c.type === "text") log.push(`${at} text ${c.text.slice(0, 200)}`);
  }
  if (m.type === "user") for (const c of (Array.isArray(m.message.content) ? m.message.content : [])) if (c.type === "tool_result") log.push(`${at} tool_result ${JSON.stringify(c.content).slice(0, 200)}`);
  if (m.type === "result") { results++; log.push(`${at} result ${m.subtype}`); if (pushed) close(); }
}
console.log(log.join("\n"));
process.exit(0);
