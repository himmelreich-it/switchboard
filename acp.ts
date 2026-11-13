// The agent driver: Agent Client Protocol (ACP) over stdio, one agent process per live session.
// Nothing about TODOs here; runner.ts decides what a session is for. See designs/acp/ (investigation, design, spike).
import * as acp from "@agentclientprotocol/sdk";
import { attachedImages, descendants } from "./lib";

export type Update = acp.SessionUpdate & { _meta?: Record<string, any> };
export type PermissionRequest = acp.RequestPermissionRequest;
export type PermissionOutcome = acp.RequestPermissionResponse["outcome"];
export type McpServer = acp.McpServer;
export type Command = { name: string; description: string; hint: string | null };
// How to start an agent: its command line and extra environment (agents.ts builds it from the registry).
// `questions`: the app takes the agent's questions as ACP form elicitations (Codex's request_user_input). Off for Claude,
// whose workers ask through the app tools (the contract), not with AskUserQuestion.
export type Spec = { command: string[]; env?: Record<string, string>; questions?: boolean };
// `key`: the method takes an API key from the user (`_meta["api-key"]`, Codex's API key login).
export type AuthMethod = { id: string; name: string; description: string | null; key?: boolean };

// session/new refused for want of a login (ACP error -32000): the agent's ways to log in, from its initialize answer.
export class AuthRequired extends Error {
  constructor(readonly methods: AuthMethod[]) { super("the agent needs a login"); }
}
const authMethodsOf = (init: any): AuthMethod[] =>
  (init?.authMethods ?? []).map((m: any) => ({ id: String(m.id), name: String(m.name ?? m.id), description: m.description ?? null, ...(m._meta?.["api-key"] ? { key: true } : {}) }));

export type Handlers = {
  onUpdate(u: Update): void;
  onSdk?(m: any): void; // a raw SDK message the session asked for in `_meta.claudeCode.emitRawSDKMessages` (Claude only)
  onPermission(r: PermissionRequest): Promise<PermissionOutcome> | PermissionOutcome;
  // A form elicitation (Spec.questions): the answer, or decline/cancel. `signal` aborts when the agent stops waiting.
  onQuestion?(r: acp.CreateElicitationRequest, signal: AbortSignal): Promise<acp.CreateElicitationResponse>;
};
export type SessionOpts = {
  cwd: string;
  additionalDirectories?: string[];
  mcpServers: McpServer[];
  meta?: Record<string, unknown>; // the agent's own `_meta` for session/new and session/resume
  // Appended to the agent's system prompt, for an agent that declares `sessionCapabilities.systemPrompt` (Codex).
  systemPrompt?: string;
  // The new session's title, for an agent that takes one (`sessionCapabilities.title`, Codex): Codex lists it in
  // `codex resume`, and skips the extra model turn it runs to make a title of its own.
  title?: string;
  env?: Record<string, string>; // added to the agent process's environment
};

export type Live = {
  sessionId: string;
  // The agent took SessionOpts.systemPrompt; false: the caller sends the instructions with the first prompt instead.
  readonly systemPrompt: boolean;
  // The session's config options (mode, model, effort, …) as the agent last reported them: from session/new or
  // session/resume, a set_config_option answer, or a config_option_update. A model switch rewrites the others.
  readonly configOptions: any[];
  // The agent's slash commands, from its last available_commands_update (empty until the first one).
  readonly commands: Command[];
  // Resolves when the turn ends: the prompt response is the end of the turn (ACP v1). Rejects on a protocol error or exit.
  prompt(text: string): Promise<acp.StopReason>;
  // Hands a message to the running turn (`_session/steering`, item 20). True when the turn took it; false when no
  // turn runs or the agent cannot steer, and the message should go out as the next prompt.
  steer(text: string): Promise<boolean>;
  cancel(): void;
  setConfig(configId: string, value: string): Promise<void>;
  // SIGTERM, so the agent can stop the processes it started; SIGKILL if it is still there 5 s later.
  close(): void;
  readonly alive: boolean;
  readonly exited: Promise<unknown>; // settles when the agent process has exited, for whatever reason
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
    .onRequest(acp.methods.client.elicitation.create, async (ctx) =>
      ctx.params.mode === "form" && handlers.h?.onQuestion ? handlers.h.onQuestion(ctx.params, ctx.signal) : { action: "cancel" as const })
    .onNotification(acp.methods.client.session.update, (ctx) => {
      const u: any = ctx.params.update;
      if (u.sessionUpdate === "config_option_update" && Array.isArray(u.configOptions)) state.configOptions = u.configOptions;
      if (u.sessionUpdate === "current_mode_update")
        state.configOptions = state.configOptions.map((o) => (o.id === "mode" ? { ...o, currentValue: u.currentModeId } : o));
      if (u.sessionUpdate === "available_commands_update" && Array.isArray(u.availableCommands))
        state.commands = u.availableCommands.map((c: any) => ({ name: String(c.name), description: String(c.description ?? ""), hint: c.input?.hint ?? null }));
      handlers.h?.onUpdate(u as Update);
    })
    .onNotification("_claude/sdkMessage", (p: unknown) => p as { message?: unknown }, (ctx) => handlers.h?.onSdk?.(ctx.params.message))
    .connect(acp.ndJsonStream(input, proc.stdout as ReadableStream<Uint8Array>));
  proc.exited.then((code) => conn.close(new Error(`the agent process exited (code ${code})${log ? ": " + lastLine(log) : ""}`)));
  try {
    const init = await withTimeout(conn.agent.request(acp.methods.agent.initialize, {
      protocolVersion: acp.PROTOCOL_VERSION,
      clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false, ...(spec.questions ? { elicitation: { form: {} } } : {}) },
      clientInfo: { name: "switchboard", version: "1" },
    }), INIT_TIMEOUT, "starting the agent");
    return { proc, conn, init, state, log: () => log };
  } catch (e) {
    proc.kill();
    throw e;
  }
}

const lastLine = (log: string) => log.trim().split("\n").at(-1)!.slice(0, 300);

// Ends a process and every process below it: `signal` now, SIGKILL 5 s later to the ones still there. The tree is read
// first, because a shell's jobs, nohup or disowned commands and Claude Code's background shells (item 136) each run in
// their own process group, and move to pid 1 once their parent ends. Windows has no signals: taskkill /T /F at once.
// ponytail: a daemon that already left the tree (parent pid 1) is missed; the SIGKILL goes to the pids read at the start.
// Async: taskkill and ps take 50-300 ms, which a sync spawn would block the server for. Resolves once `signal` is sent.
export async function killTree(pid: number, signal: NodeJS.Signals) {
  if (process.platform === "win32") {
    try { await Bun.spawn(["taskkill", "/PID", String(pid), "/T", "/F"], { stdin: "ignore", stdout: "ignore", stderr: "ignore", windowsHide: true }).exited; } catch {}
    return;
  }
  let pids = [pid];
  try {
    const ps = Bun.spawn(["/bin/ps", "-A", "-o", "pid=,ppid="], { stdin: "ignore", stdout: "pipe", stderr: "ignore" });
    pids = [pid, ...descendants(await new Response(ps.stdout).text(), pid)];
  } catch {}
  const send = (s: NodeJS.Signals) => { for (const p of pids) try { process.kill(p, s); } catch {} };
  send(signal);
  const kill = () => { clearTimeout(timer); pendingKills.delete(kill); send("SIGKILL"); };
  const timer = setTimeout(kill, 5000);
  pendingKills.add(kill);
}
// The SIGKILLs killTree has not sent yet. The server's shutdown sends them at once: its exit would drop the timers.
const pendingKills = new Set<() => void>();
export const killNow = () => { for (const kill of [...pendingKills]) kill(); };

function live(a: Awaited<ReturnType<typeof spawnAgent>>, sessionId: string, configOptions: any[] | null | undefined, systemPrompt: boolean): Live {
  let alive = true;
  a.proc.exited.then(() => (alive = false));
  if (configOptions) a.state.configOptions = configOptions;
  return {
    sessionId,
    systemPrompt,
    get configOptions() { return a.state.configOptions; },
    get commands() { return a.state.commands; },
    async prompt(text) {
      try {
        const r = await a.conn.agent.request(acp.methods.agent.session.prompt, { sessionId, prompt: [{ type: "text", text }, ...(await imageBlocks(a.init, text))] });
        return r.stopReason;
      } catch (e) {
        // The connection can close on the agent's stdout ending before the exit is seen: name the exit, not the closed connection.
        if (!a.conn.signal.aborted) throw e;
        const code = await Promise.race([a.proc.exited, Bun.sleep(1000).then(() => null)]);
        throw new Error(`the agent process ${code === null ? "stopped answering" : `exited (code ${code})`}${a.log() ? ": " + lastLine(a.log()) : ""}`);
      }
    },
    async steer(text) {
      if (!(a.init as any)._meta?.steering?.supported) return false;
      const r: any = await a.conn.agent.request("_session/steering" as any, {
        sessionId, prompt: [{ type: "text", text }], _meta: { steering: { idleBehavior: "promptRequired" } },
      } as any);
      return r?.outcome === "injected";
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
      alive = false;
      // The connection closes after the tree is read: the agent may exit on it, and its children would no longer be found below it.
      void killTree(a.proc.pid, "SIGTERM").finally(() => a.conn.close());
    },
    // Dead as soon as either its process exited or its connection closed, whichever is seen first.
    get alive() { return alive && !a.conn.signal.aborted && a.proc.exitCode === null; },
    exited: a.proc.exited,
    stderr: a.log,
  };
}

// The attached images as image blocks, for an agent that takes them (Codex); their paths stay in the text. Claude reads
// them with its Read tool as before. An image that cannot be read is left to the path.
async function imageBlocks(init: any, text: string): Promise<acp.ContentBlock[]> {
  if (!init.agentCapabilities?.promptCapabilities?.image || init.agentInfo?.name === "@agentclientprotocol/claude-agent-acp") return [];
  const blocks: acp.ContentBlock[] = [];
  for (const path of attachedImages(text)) {
    const f = Bun.file(path);
    if (f.size > 0 && f.size <= 20 * 1024 * 1024) blocks.push({ type: "image", mimeType: f.type, data: Buffer.from(await f.arrayBuffer()).toString("base64"), uri: `file://${path}` });
  }
  return blocks;
}

// ACP's `systemPrompt` (append) for session/new and session/resume, when the caller has one and the agent takes it.
const systemPromptOf = (init: any, o: SessionOpts) =>
  o.systemPrompt && init.agentCapabilities?.sessionCapabilities?.systemPrompt
    ? { systemPrompt: { mode: "append", content: [{ type: "text", text: o.systemPrompt }] } } : {};

// session/new in a new agent process. The agent makes the session id; for Claude it is the Claude Code session id.
export async function openSession(spec: Spec, o: SessionOpts, h: Handlers): Promise<Live> {
  const handlers = { h };
  const a = await spawnAgent(spec, handlers, o.env);
  try {
    const sp = systemPromptOf(a.init, o);
    const s = await a.conn.agent.request(acp.methods.agent.session.new, {
      cwd: o.cwd, mcpServers: o.mcpServers, additionalDirectories: o.additionalDirectories, _meta: o.meta, ...sp,
    } as acp.NewSessionRequest);
    if (o.title && (a.init.agentCapabilities as any)?.sessionCapabilities?.title)
      await a.conn.agent.request("session/set_title" as any, { sessionId: s.sessionId, title: o.title }).catch(() => {});   // a name only
    return live(a, s.sessionId, s.configOptions, !!sp.systemPrompt);
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
    const sp = systemPromptOf(a.init, o);
    const params = { sessionId, cwd: o.cwd, mcpServers: o.mcpServers, additionalDirectories: o.additionalDirectories, _meta: o.meta, ...sp };
    let s: any;
    if (caps.sessionCapabilities?.resume) s = await a.conn.agent.request(acp.methods.agent.session.resume, params as any);
    else if (caps.loadSession) s = await a.conn.agent.request(acp.methods.agent.session.load, params as any);
    else throw new Error("this agent cannot resume sessions");
    handlers.h = h;
    return live(a, sessionId, s?.configOptions, !!sp.systemPrompt);
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

// ACP authenticate in a short-lived agent process. The agent does the login itself (codex-acp opens the browser for
// ChatGPT and stores the login in ~/.codex/, P5 probe); this resolves when it is done, false when the agent reports a failure.
export async function authenticate(spec: Spec, methodId: string, apiKey?: string, timeoutMs = 5 * 60_000): Promise<boolean> {
  const a = await spawnAgent(spec, { h: null });
  try {
    const params = { methodId, ...(apiKey ? { _meta: { "api-key": { apiKey } } } : {}) };
    const r: any = await withTimeout(a.conn.agent.request(acp.methods.agent.authenticate, params as any), timeoutMs, "the login");
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
