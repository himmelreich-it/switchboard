// The agent driver: Agent Client Protocol (ACP) over stdio, one agent process per live session.
// Nothing about TODOs here; runner.ts decides what a session is for. See designs/acp-design.md and acp-spike.md.
import * as acp from "@agentclientprotocol/sdk";
import { dirname, join } from "node:path";

export type Update = acp.SessionUpdate & { _meta?: Record<string, any> };
export type PermissionRequest = acp.RequestPermissionRequest;
export type PermissionOutcome = acp.RequestPermissionResponse["outcome"];
export type McpServer = acp.McpServer;

export type Handlers = {
  onUpdate(u: Update): void;
  onPermission(r: PermissionRequest): Promise<PermissionOutcome> | PermissionOutcome;
};
export type SessionOpts = {
  cwd: string;
  additionalDirectories?: string[];
  mcpServers: McpServer[];
  meta?: Record<string, unknown>; // the agent's own `_meta` for session/new and session/resume
  env?: Record<string, string>; // added to the agent process's environment
};

export type Live = {
  sessionId: string;
  configOptions: any[];
  // Resolves when the turn ends: the prompt response is the end of the turn (ACP v1). Rejects on a protocol error or exit.
  prompt(text: string): Promise<acp.StopReason>;
  cancel(): void;
  setConfig(configId: string, value: string): Promise<void>;
  // SIGTERM, so the agent can stop the processes it started; SIGKILL if it is still there 5 s later.
  close(): void;
  readonly alive: boolean;
  stderr(): string; // the agent's last log lines, for error events
};

// The Claude adapter runs on the Bun that runs this server (verified in the spike): no Node needed.
const CLAUDE = [process.execPath, join(dirname(Bun.resolveSync("@agentclientprotocol/claude-agent-acp/package.json", import.meta.dir)), "dist/index.js")];
const INIT_TIMEOUT = 30_000;

const withTimeout = <T>(p: Promise<T>, ms: number, what: string) =>
  Promise.race([p, new Promise<never>((_, rej) => setTimeout(() => rej(new Error(`${what} timed out after ${ms / 1000} s`)), ms))]);

// One agent process and its ACP connection, initialized. `handlers` may be set later (session/load replays before
// the caller has a Live), and stays swappable so the same process serves the next turn's run.
async function spawnAgent(handlers: { h: Handlers | null }, extraEnv: Record<string, string> = {}) {
  const proc = Bun.spawn(CLAUDE, { stdin: "pipe", stdout: "pipe", stderr: "pipe", env: { ...process.env, ...extraEnv } });
  let log = "";
  (async () => {
    const dec = new TextDecoder(), r = proc.stderr.getReader();
    for (let c = await r.read(); !c.done; c = await r.read()) log = (log + dec.decode(c.value)).slice(-8000);
  })().catch(() => {});
  const input = new WritableStream<Uint8Array>({
    write(c) { proc.stdin.write(c); proc.stdin.flush(); },
    close() { proc.stdin.end(); },
  });
  const conn = acp
    .client({ name: "switchboard" })
    .onRequest(acp.methods.client.session.requestPermission, async (ctx) => ({
      outcome: handlers.h ? await handlers.h.onPermission(ctx.params) : { outcome: "cancelled" as const },
    }))
    .onNotification(acp.methods.client.session.update, (ctx) => handlers.h?.onUpdate(ctx.params.update as Update))
    .connect(acp.ndJsonStream(input, proc.stdout as ReadableStream<Uint8Array>));
  proc.exited.then((code) => conn.close(new Error(`the agent process exited (code ${code})${log ? ": " + lastLine(log) : ""}`)));
  try {
    const init = await withTimeout(conn.agent.request(acp.methods.agent.initialize, {
      protocolVersion: acp.PROTOCOL_VERSION,
      clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false },
      clientInfo: { name: "switchboard", version: "1" },
    }), INIT_TIMEOUT, "starting the agent");
    return { proc, conn, init, log: () => log };
  } catch (e) {
    proc.kill();
    throw e;
  }
}

const lastLine = (log: string) => log.trim().split("\n").at(-1)!.slice(0, 300);

function live(a: Awaited<ReturnType<typeof spawnAgent>>, sessionId: string, configOptions: any[] | null | undefined): Live {
  let alive = true;
  a.proc.exited.then(() => (alive = false));
  return {
    sessionId,
    configOptions: configOptions ?? [],
    async prompt(text) {
      const r = await a.conn.agent.request(acp.methods.agent.session.prompt, { sessionId, prompt: [{ type: "text", text }] });
      return r.stopReason;
    },
    cancel() {
      a.conn.agent.notify(acp.methods.agent.session.cancel, { sessionId }).catch(() => {});
    },
    async setConfig(configId, value) {
      const r: any = await a.conn.agent.request(acp.methods.agent.session.setConfigOption, { sessionId, configId, value } as any);
      if (r?.configOptions) this.configOptions = r.configOptions;
    },
    close() {
      if (!alive) return;
      a.proc.kill("SIGTERM");
      setTimeout(() => alive && a.proc.kill("SIGKILL"), 5000);
    },
    get alive() { return alive; },
    stderr: a.log,
  };
}

// session/new in a new agent process. The agent makes the session id; for Claude it is the Claude Code session id.
export async function openSession(o: SessionOpts, h: Handlers): Promise<Live> {
  const handlers = { h };
  const a = await spawnAgent(handlers, o.env);
  try {
    const s = await a.conn.agent.request(acp.methods.agent.session.new, {
      cwd: o.cwd, mcpServers: o.mcpServers, additionalDirectories: o.additionalDirectories, _meta: o.meta,
    } as acp.NewSessionRequest);
    return live(a, s.sessionId, s.configOptions);
  } catch (e) {
    a.proc.kill();
    throw e;
  }
}

// An existing session in a new agent process: session/resume, or session/load for agents without resume (its replayed
// history is dropped: the app keeps its own record). MCP servers and `_meta` are given again, so they may differ.
export async function reopenSession(sessionId: string, o: SessionOpts, h: Handlers): Promise<Live> {
  const handlers: { h: Handlers | null } = { h: null };
  const a = await spawnAgent(handlers, o.env);
  try {
    const caps: any = a.init.agentCapabilities ?? {};
    const params = { sessionId, cwd: o.cwd, mcpServers: o.mcpServers, additionalDirectories: o.additionalDirectories, _meta: o.meta };
    let s: any;
    if (caps.sessionCapabilities?.resume) s = await a.conn.agent.request(acp.methods.agent.session.resume, params as any);
    else if (caps.loadSession) s = await a.conn.agent.request(acp.methods.agent.session.load, params as any);
    else throw new Error("this agent cannot resume sessions");
    handlers.h = h;
    return live(a, sessionId, s?.configOptions);
  } catch (e) {
    a.proc.kill();
    throw e;
  }
}

// Whether the agent still has the session (the restore check). An agent that cannot list sessions gets the benefit of
// the doubt: the resume itself then reports the problem.
export async function sessionExists(cwd: string, sessionId: string): Promise<boolean> {
  const a = await spawnAgent({ h: null });
  try {
    if (!(a.init.agentCapabilities as any)?.sessionCapabilities?.list) return true;
    let cursor: string | undefined;
    for (let page = 0; page < 50; page++) {
      const r: any = await a.conn.agent.request(acp.methods.agent.session.list, { cwd, cursor } as any);
      if (r.sessions?.some((s: any) => s.sessionId === sessionId)) return true;
      if (!(cursor = r.nextCursor)) return false;
    }
    return true;
  } finally {
    a.proc.kill();
  }
}
