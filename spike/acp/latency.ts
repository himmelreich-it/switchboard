import { query } from "@anthropic-ai/claude-agent-sdk";
import { spawnAgent, startMcp, textOf } from "./harness";
const CWD = "/tmp/sb-acp/repo", P = "Reply with just the word hi.";
const mcp = startMcp();
const out: any = { sdk: [], acp: [] };
for (let i = 0; i < 2; i++) {
  let t = performance.now(), first = 0;
  for await (const m of query({ prompt: P, options: { cwd: CWD, permissionMode: "auto", includePartialMessages: true, systemPrompt: { type: "preset", preset: "claude_code" } } }) as any) {
    if (!first && (m.type === "stream_event" || m.type === "assistant")) first = performance.now() - t;
    if (m.type === "result") break;
  }
  out.sdk.push({ firstMs: Math.round(first), totalMs: Math.round(performance.now() - t) });
  t = performance.now();
  const a = await spawnAgent("lat");
  const s = await a.req("session/new", { cwd: CWD, mcpServers: [] });
  await a.req("session/set_config_option", { sessionId: s.sessionId, configId: "mode", value: "auto" });
  let f2 = 0; const n = a.updates.length;
  const iv = setInterval(() => { if (!f2 && textOf(a.updates, n)) f2 = performance.now() - t; }, 10);
  await a.req("session/prompt", { sessionId: s.sessionId, prompt: [{ type: "text", text: P }] });
  clearInterval(iv);
  out.acp.push({ firstMs: Math.round(f2), totalMs: Math.round(performance.now() - t), spawnInitMs: Math.round(a.spawnMs) });
  a.kill();
}
console.log(JSON.stringify(out));
mcp.stop(true); process.exit(0);
