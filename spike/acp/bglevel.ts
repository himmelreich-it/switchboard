// Background level spike (2026-10-11): does the adapter forward the SDK's background_tasks_changed when asked for it
// with emitRawSDKMessages, and does a task killed by a plain `kill` (not TaskStop) leave the level?
// Run from the repo root: bun spike/acp/bglevel.ts
import * as acp from "@agentclientprotocol/sdk";

const bin = `${import.meta.dir}/../../node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js`;
const proc = Bun.spawn([process.execPath, bin], { stdin: "pipe", stdout: "pipe", stderr: "inherit" });
const input = new WritableStream<Uint8Array>({ write(c) { proc.stdin.write(c); proc.stdin.flush(); }, close() { proc.stdin.end(); } });
const t0 = Date.now();
const at = () => ((Date.now() - t0) / 1000).toFixed(1);
const conn = acp
  .client({ name: "bglevel" })
  .onRequest(acp.methods.client.session.requestPermission, async (ctx) => ({
    outcome: { outcome: "selected" as const, optionId: ctx.params.options.find((o) => o.kind.startsWith("allow"))!.optionId },
  }))
  .onNotification(acp.methods.client.session.update, (ctx) => {
    const u: any = ctx.params.update;
    if (u.sessionUpdate === "tool_call_update" && u._meta?.claudeCode?.toolResponse)
      console.log(at(), "tool", u._meta.claudeCode.toolName, JSON.stringify(u._meta.claudeCode.toolResponse).slice(0, 160));
    if (u.sessionUpdate === "usage_update") console.log(at(), "usage origin", u._meta?.["_claude/origin"]?.kind ?? "-");
  })
  .onNotification("_claude/sdkMessage", (p: unknown) => p as any, (ctx: any) => {
    const m = ctx.params.message;
    console.log(at(), "SDK", m.type, m.subtype, JSON.stringify(m.tasks?.map((t: any) => [t.task_id, t.task_type, t.ambient])));
  })
  .connect(acp.ndJsonStream(input, proc.stdout as ReadableStream<Uint8Array>));
await conn.agent.request(acp.methods.agent.initialize, {
  protocolVersion: acp.PROTOCOL_VERSION, clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false },
});
const s: any = await conn.agent.request(acp.methods.agent.session.new, {
  cwd: "/tmp", mcpServers: [],
  _meta: { claudeCode: { emitRawSDKMessages: [{ type: "system", subtype: "background_tasks_changed" }], options: { allowedTools: ["Bash"] } } },
});
const prompt = (text: string) => conn.agent.request(acp.methods.agent.session.prompt, { sessionId: s.sessionId, prompt: [{ type: "text", text }] });
console.log(at(), "turn 1", (await prompt(
  "Run exactly these Bash calls, nothing else: 1) `sleep 600` with run_in_background true. 2) `sleep 300` with run_in_background true. " +
  "3) in the foreground: `sleep 2; pkill -f 'slee[p] 600'`. Then reply DONE.")).stopReason);
await Bun.sleep(20_000);
console.log(at(), "turn 2", (await prompt("In the foreground run `pkill -f 'slee[p] 300'`, then reply DONE.")).stopReason);
await Bun.sleep(20_000);
console.log(at(), "end");
proc.kill();
process.exit(0);
