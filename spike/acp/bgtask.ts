// Background task spike: what the adapter sends after end_turn when a backgrounded Bash finishes.
import { spawnAgent, startMcp, mcpFor } from "./harness";

const a = await spawnAgent("bgtask");

const s = await a.req("session/new", { cwd: process.cwd(), mcpServers: [] });
const t0 = Date.now();
const short = (u: any) => {
  const x = u.update;
  const k = x.sessionUpdate;
  if (k === "agent_message_chunk" || k === "agent_thought_chunk") return `${k} ${JSON.stringify(x.content?.text).slice(0, 80)}`;
  if (k === "tool_call" || k === "tool_call_update") return `${k} ${x.toolCallId} ${x.status ?? ""} ${x.title ?? ""} raw=${JSON.stringify(x.rawOutput ?? x.rawInput ?? null).slice(0, 300)} meta=${JSON.stringify(x._meta ?? null).slice(0, 300)}`;
  return `${k} ${JSON.stringify(x).slice(0, 300)}`;
};
let seen = 0;
const dump = (tag: string) => { for (; seen < a.updates.length; seen++) console.log(((Date.now() - t0) / 1000).toFixed(1), tag, short(a.updates[seen])); };
const tick = setInterval(() => dump("."), 250);
const r = await a.req("session/prompt", { sessionId: s.sessionId, prompt: [{ type: "text", text: process.argv[2] }] });
dump(".");
console.log(((Date.now() - t0) / 1000).toFixed(1), "PROMPT RESOLVED", JSON.stringify(r));
await Bun.sleep(60_000);
dump(".");
clearInterval(tick);
a.kill(); process.exit(0);
