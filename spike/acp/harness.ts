// P0 spike harness: spawns claude-agent-acp under Bun, talks ACP over stdio, and serves a test "app" MCP server over HTTP.
import * as acp from "@agentclientprotocol/sdk";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { WebStandardStreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/webStandardStreamableHttp.js";
import { appendFileSync } from "node:fs";
import { z } from "zod";

export const LOG = `${import.meta.dir}/out/log.jsonl`;
export const log = (tag: string, data: unknown) => appendFileSync(LOG, JSON.stringify({ at: Date.now(), tag, data }) + "\n");

// ---------- app MCP server over HTTP, keyed by a per-session secret ----------
export const toolCalls: { secret: string; name: string; args: any }[] = [];
export const secrets = new Map<string, string>(); // secret -> label
export const mcpHits: { secret: string | null; method: string }[] = [];

function appServer(secret: string) {
  const s = new McpServer({ name: "app", version: "0.0.0" });
  s.registerTool("post_message", { description: "Post a one or two line chat message to the user.", inputSchema: { body: z.string() } }, async (args) => {
    toolCalls.push({ secret, name: "post_message", args });
    return { content: [{ type: "text", text: "ok" }] };
  });
  s.registerTool("set_title", { description: "Set this TODO's title.", inputSchema: { title: z.string() } }, async (args) => {
    toolCalls.push({ secret, name: "set_title", args });
    return { content: [{ type: "text", text: "ok" }] };
  });
  return s;
}

export function startMcp(port = 4499) {
  return Bun.serve({
    hostname: "127.0.0.1",
    port,
    async fetch(req) {
      const url = new URL(req.url);
      if (url.pathname !== "/mcp/app") return new Response("not found", { status: 404 });
      const secret = req.headers.get("x-switchboard-session");
      let method = req.method;
      try { if (req.method === "POST") method += " " + ((await req.clone().json()) as any).method; } catch {}
      mcpHits.push({ secret, method });
      if (!secret || !secrets.has(secret)) return new Response("forbidden", { status: 403 });
      if (req.method !== "POST") return new Response(null, { status: 405 }); // stateless: no GET SSE stream
      const transport = new WebStandardStreamableHTTPServerTransport({ sessionIdGenerator: undefined, enableJsonResponse: true });
      await appServer(secret).connect(transport);
      return transport.handleRequest(req);
    },
  });
}

// ---------- ACP client ----------
export type Agent = Awaited<ReturnType<typeof spawnAgent>>;
export type PermissionPolicy = (req: acp.RequestPermissionRequest) => Promise<acp.RequestPermissionResponse> | acp.RequestPermissionResponse;

export async function spawnAgent(label: string, opts: { permission?: PermissionPolicy; env?: Record<string, string> } = {}) {
  const t0 = performance.now();
  const bin = `${import.meta.dir}/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js`;
  const proc = Bun.spawn(["bun", bin], { stdin: "pipe", stdout: "pipe", stderr: "pipe", env: { ...process.env, ...opts.env } });
  (async () => { for await (const c of proc.stderr) appendFileSync(`${import.meta.dir}/out/${label}.stderr`, c); })();
  const input = new WritableStream<Uint8Array>({ write(c) { proc.stdin.write(c); proc.stdin.flush(); }, close() { proc.stdin.end(); } });
  const stream = acp.ndJsonStream(input, proc.stdout as ReadableStream<Uint8Array>);
  const updates: acp.SessionNotification[] = [];
  const permissions: acp.RequestPermissionRequest[] = [];
  const ext: { method: string; params: unknown }[] = [];
  let policy: PermissionPolicy = opts.permission ?? ((r) => ({ outcome: { outcome: "selected", optionId: r.options.find((o) => o.kind === "allow_once")!.optionId } }));
  const conn = acp
    .client({ name: "switchboard-spike" })
    .onRequest(acp.methods.client.session.requestPermission, async (ctx) => {
      permissions.push(ctx.params);
      log(`${label}:permission`, ctx.params);
      return policy(ctx.params);
    })
    .onNotification(acp.methods.client.session.update, (ctx) => {
      updates.push(ctx.params);
      log(`${label}:update`, ctx.params);
    })
    .connect(stream);
  const init = await conn.agent.request(acp.methods.agent.initialize, {
    protocolVersion: acp.PROTOCOL_VERSION,
    clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false, ...(process.env.AIR ? { _meta: { jetbrains: { air: { version: 1, capabilities: ["asyncTasks"] } } } } : {}) },
    clientInfo: { name: "switchboard-spike", version: "0.0.0" },
  });
  log(`${label}:initialize`, init);
  return {
    label, proc, conn, init, updates, permissions, ext,
    spawnMs: performance.now() - t0,
    setPolicy(p: PermissionPolicy) { policy = p; },
    req: <T = any>(method: string, params: any) => conn.agent.request(method as any, params) as Promise<T>,
    kill() { conn.close(); proc.kill(); },
  };
}

export function mcpFor(label: string) {
  const secret = crypto.randomUUID();
  secrets.set(secret, label);
  return { secret, server: { type: "http" as const, name: "app", url: "http://127.0.0.1:4499/mcp/app", headers: [{ name: "X-Switchboard-Session", value: secret }] } };
}

export const textOf = (ups: acp.SessionNotification[], from = 0) =>
  ups.slice(from).map((u) => u.update).filter((u: any) => u.sessionUpdate === "agent_message_chunk" && u.content.type === "text").map((u: any) => u.content.text).join("");
