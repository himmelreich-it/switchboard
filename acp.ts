// The agent driver: Agent Client Protocol (ACP) over stdio, one agent process per live session.
// Nothing about TODOs here; runner.ts decides what a session is for. See designs/acp/ (investigation, design, spike).
import * as acp from "@agentclientprotocol/sdk";

export type Update = acp.SessionUpdate & { _meta?: Record<string, any> };
export type PermissionRequest = acp.RequestPermissionRequest;
export type PermissionOutcome = acp.RequestPermissionResponse["outcome"];
export type McpServer = acp.McpServer;
export type Command = { name: string; description: string; hint: string | null };
// How to start an agent: its command line and extra environment (agents.ts builds it from the registry).
export type Spec = { command: string[]; env?: Record<string, string> };
export type AuthMethod = { id: string; name: string; description: string | null };

// session/new refused for want of a login (ACP error -32000): the agent's ways to log in, from its initialize answer.
export class AuthRequired extends Error {
  constructor(readonly methods: AuthMethod[]) { super("the agent needs a login"); }
}
const authMethodsOf = (init: any): AuthMethod[] =>
  (init?.authMethods ?? []).map((m: any) => ({ id: String(m.id), name: String(m.name ?? m.id), description: m.description ?? null }));

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
  // The session's config options (mode, model, effort, …) as the agent last reported them: from session/new or
  // session/resume, a set_config_option answer, or a config_option_update. A model switch rewrites the others.
  readonly configOptions: any[];
  // The agent's slash commands, from its last available_commands_update (empty until the first one).
  readonly commands: Command[];
  // Resolves when the turn ends: the prompt response is the end of the turn (ACP v1). Rejects on a protocol error or exit.
  prompt(text: string): Promise<acp.StopReason>;
  cancel(): void;
  setConfig(configId: string, value: string): Promise<void>;
  // SIGTERM, so the agent can stop the processes it started; SIGKILL if it is still there 5 s later.
  close(): void;
  readonly alive: boolean;
  stderr(): string; // the agent's last log lines, for error events
};

const INIT_TIMEOUT = 30_000;

const withTimeout = <T>(p: Promise<T>, ms: number, what: string) =>
  Promise.race([p, new Promise<never>((_, rej) => setTimeout(() => rej(new Error(`${what} timed out after ${ms / 1000} s`)), ms))]);

// One agent process and its ACP connection, initialized. `handlers` may be set later (session/load replays before
// the caller has a Live), and stays swappable so the same process serves the next turn's run.
async function spawnAgent(spec: Spec, handlers: { h: Handlers | null }, extraEnv: Record<string, string> = {}) {
  const state = { configOptions: [] as any[], commands: [] as Command[] };
  let proc: ReturnType<typeof Bun.spawn<"pipe", "pipe", "pipe">>;
  try {
    proc = Bun.spawn(spec.command, { stdin: "pipe", stdout: "pipe", stderr: "pipe", env: { ...process.env, ...spec.env, ...extraEnv }, windowsHide: true });
  } catch (e: any) {
    throw new Error(`could not run ${spec.command[0]}: ${e?.message ?? e}`);
  }
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
    .onNotification(acp.methods.client.session.update, (ctx) => {
      const u: any = ctx.params.update;
      if (u.sessionUpdate === "config_option_update" && Array.isArray(u.configOptions)) state.configOptions = u.configOptions;
      if (u.sessionUpdate === "current_mode_update")
        state.configOptions = state.configOptions.map((o) => (o.id === "mode" ? { ...o, currentValue: u.currentModeId } : o));
      if (u.sessionUpdate === "available_commands_update" && Array.isArray(u.availableCommands))
        state.commands = u.availableCommands.map((c: any) => ({ name: String(c.name), description: String(c.description ?? ""), hint: c.input?.hint ?? null }));
      handlers.h?.onUpdate(u as Update);
    })
    .connect(acp.ndJsonStream(input, proc.stdout as ReadableStream<Uint8Array>));
  proc.exited.then((code) => conn.close(new Error(`the agent process exited (code ${code})${log ? ": " + lastLine(log) : ""}`)));
  try {
    const init = await withTimeout(conn.agent.request(acp.methods.agent.initialize, {
      protocolVersion: acp.PROTOCOL_VERSION,
      clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false },
      clientInfo: { name: "switchboard", version: "1" },
    }), INIT_TIMEOUT, "starting the agent");
    return { proc, conn, init, state, log: () => log };
  } catch (e) {
    proc.kill();
    throw e;
  }
}

const lastLine = (log: string) => log.trim().split("\n").at(-1)!.slice(0, 300);

function live(a: Awaited<ReturnType<typeof spawnAgent>>, sessionId: string, configOptions: any[] | null | undefined): Live {
  let alive = true;
  a.proc.exited.then(() => (alive = false));
  if (configOptions) a.state.configOptions = configOptions;
  return {
    sessionId,
    get configOptions() { return a.state.configOptions; },
    get commands() { return a.state.commands; },
    async prompt(text) {
      try {
        const r = await a.conn.agent.request(acp.methods.agent.session.prompt, { sessionId, prompt: [{ type: "text", text }] });
        return r.stopReason;
      } catch (e) {
        // The connection can close on the agent's stdout ending before the exit is seen: name the exit, not the closed connection.
        if (!a.conn.signal.aborted) throw e;
        const code = await Promise.race([a.proc.exited, Bun.sleep(1000).then(() => null)]);
        throw new Error(`the agent process ${code === null ? "stopped answering" : `exited (code ${code})`}${a.log() ? ": " + lastLine(a.log()) : ""}`);
      }
    },
    cancel() {
      a.conn.agent.notify(acp.methods.agent.session.cancel, { sessionId }).catch(() => {});
    },
    async setConfig(configId, value) {
      const r: any = await a.conn.agent.request(acp.methods.agent.session.setConfigOption, { sessionId, configId, value } as any);
      if (r?.configOptions) a.state.configOptions = r.configOptions;
    },
    close() {
      if (!alive) return;
      a.conn.close();
      a.proc.kill("SIGTERM");
      setTimeout(() => alive && a.proc.kill("SIGKILL"), 5000);
    },
    // Dead as soon as either its process exited or its connection closed, whichever is seen first.
    get alive() { return alive && !a.conn.signal.aborted && a.proc.exitCode === null; },
    stderr: a.log,
  };
}

// session/new in a new agent process. The agent makes the session id; for Claude it is the Claude Code session id.
export async function openSession(spec: Spec, o: SessionOpts, h: Handlers): Promise<Live> {
  const handlers = { h };
  const a = await spawnAgent(spec, handlers, o.env);
  try {
    const s = await a.conn.agent.request(acp.methods.agent.session.new, {
      cwd: o.cwd, mcpServers: o.mcpServers, additionalDirectories: o.additionalDirectories, _meta: o.meta,
    } as acp.NewSessionRequest);
    return live(a, s.sessionId, s.configOptions);
  } catch (e: any) {
    a.proc.kill();
    if (e?.code === -32000) throw new AuthRequired(authMethodsOf(a.init));
    throw e;
  }
}

// An existing session in a new agent process: session/resume, or session/load for agents without resume (its replayed
// history is dropped: the app keeps its own record). MCP servers and `_meta` are given again, so they may differ.
export async function reopenSession(spec: Spec, sessionId: string, o: SessionOpts, h: Handlers): Promise<Live> {
  const handlers: { h: Handlers | null } = { h: null };
  const a = await spawnAgent(spec, handlers, o.env);
  try {
    const caps: any = a.init.agentCapabilities ?? {};
    const params = { sessionId, cwd: o.cwd, mcpServers: o.mcpServers, additionalDirectories: o.additionalDirectories, _meta: o.meta };
    let s: any;
    if (caps.sessionCapabilities?.resume) s = await a.conn.agent.request(acp.methods.agent.session.resume, params as any);
    else if (caps.loadSession) s = await a.conn.agent.request(acp.methods.agent.session.load, params as any);
    else throw new Error("this agent cannot resume sessions");
    handlers.h = h;
    return live(a, sessionId, s?.configOptions);
  } catch (e: any) {
    a.proc.kill();
    if (e?.code === -32000) throw new AuthRequired(authMethodsOf(a.init));
    throw e;
  }
}

// What an agent says about itself in initialize (Settings › Agents): its name and version, and its ways to log in.
export async function describe(spec: Spec): Promise<{ name: string | null; version: string | null; authMethods: AuthMethod[] }> {
  const a = await spawnAgent(spec, { h: null });
  a.proc.kill();
  const info: any = a.init.agentInfo ?? {};
  return { name: info.title ?? info.name ?? null, version: info.version ?? null, authMethods: authMethodsOf(a.init) };
}

// ACP authenticate in a short-lived agent process. The agent does the login itself (Codex opens the browser for
// ChatGPT and stores the login in ~/.codex/); this resolves when it is done, false when the agent reports a failure.
export async function authenticate(spec: Spec, methodId: string, timeoutMs = 5 * 60_000): Promise<boolean> {
  const a = await spawnAgent(spec, { h: null });
  try {
    const r: any = await withTimeout(a.conn.agent.request(acp.methods.agent.authenticate, { methodId } as any), timeoutMs, "the login");
    return r !== false;
  } finally {
    a.proc.kill();
  }
}

// Whether the agent still has the session (the restore check). An agent that cannot list sessions gets the benefit of
// the doubt: the resume itself then reports the problem.
export async function sessionExists(spec: Spec, cwd: string, sessionId: string): Promise<boolean> {
  const a = await spawnAgent(spec, { h: null });
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
