import * as acp from "@agentclientprotocol/sdk";
const entry = process.argv[2];
const args = process.argv.slice(3);
const proc = Bun.spawn([process.execPath, entry, ...args], { stdin: "pipe", stdout: "pipe", stderr: "inherit" });
const input = new WritableStream<Uint8Array>({ write(c) { proc.stdin.write(c); proc.stdin.flush(); }, close() { proc.stdin.end(); } });
const updates: any[] = [];
const conn = acp.client({ name: "probe" })
  .onRequest(acp.methods.client.session.requestPermission, async () => ({ outcome: { outcome: "cancelled" as const } }))
  .onNotification(acp.methods.client.session.update, (ctx) => updates.push(ctx.params.update))
  .connect(acp.ndJsonStream(input, proc.stdout as ReadableStream<Uint8Array>));
const init = await conn.agent.request(acp.methods.agent.initialize, { protocolVersion: acp.PROTOCOL_VERSION, clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false }, clientInfo: { name: "probe", version: "1" } });
console.log("INIT", JSON.stringify(init, null, 1));
try {
  const s: any = await conn.agent.request(acp.methods.agent.session.new, { cwd: process.cwd(), mcpServers: [] } as any);
  console.log("NEW", s.sessionId, JSON.stringify(s.configOptions?.map((o: any) => [o.id, o.category, o.currentValue, o.options?.map((x: any) => x.value ?? x.name)])), JSON.stringify(s.modes));
} catch (e: any) { console.log("NEW ERR", e?.code, e?.message, JSON.stringify(e?.data)); }
await Bun.sleep(1000);
console.log("UPDATES", updates.map((u) => u.sessionUpdate));
proc.kill(); process.exit(0);
