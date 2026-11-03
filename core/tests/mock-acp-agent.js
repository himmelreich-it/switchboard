// A mock ACP agent for acp.rs's tests: newline-delimited JSON-RPC 2.0 on stdin/stdout, run with bun.
// MOCK_AUTH_FAIL=1 makes session/new fail with -32000 (a login is needed).
// Prompt texts: "wait" keeps the turn open until session/cancel; "die" exits with code 3 mid-turn; "tree" starts a
// child process and answers with its pid; anything else streams updates, one permission round trip and ends.

const out = (m) => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", ...m }) + "\n");
const notify = (method, params) => out({ method, params });
let nextId = 0;
const waiting = new Map();
const request = (method, params) => new Promise((res) => { const id = `m${nextId++}`; waiting.set(id, res); out({ id, method, params }); });

const options = (mode = "default") => [
  { id: "mode", name: "Mode", type: "select", currentValue: mode, options: [{ value: "default", name: "Default" }, { value: "plan", name: "Plan" }] },
  { id: "model", name: "Model", type: "select", currentValue: "m1", options: [{ value: "m1", name: "M1" }, { value: "m2", name: "M2" }] },
];
let cancelTurn = null;

async function handle(msg) {
  const { id, method, params } = msg;
  const reply = (result) => out({ id, result });
  const error = (code, message) => out({ id, error: { code, message } });
  switch (method) {
    case "initialize":
      return reply({
        protocolVersion: 1,
        agentCapabilities: { loadSession: false, sessionCapabilities: { resume: {}, list: {} } },
        agentInfo: { name: "mock-agent", title: "Mock Agent", version: "0.1.0" },
        authMethods: [{ id: "mock-login", name: "Mock login", description: "pretend to log in" }],
        _meta: { steering: { supported: true }, clientInfo: params.clientInfo, protocolVersion: params.protocolVersion },
      });
    case "session/new":
      if (process.env.MOCK_AUTH_FAIL) return error(-32000, "Authentication required");
      setTimeout(() => notify("session/update", { sessionId: "s1", update: {
        sessionUpdate: "available_commands_update",
        availableCommands: [{ name: "review", description: "Review the code", input: { hint: "what to review" } }, { name: "clear" }],
      } }), 10);
      return reply({ sessionId: "s1", configOptions: options(), _meta: { echo: params._meta ?? null, cwd: params.cwd, dirs: params.additionalDirectories, env: process.env.MOCK_EXTRA ?? null } });
    case "session/resume":
      return reply({ configOptions: options("plan") });
    case "session/list":
      if (!params.cursor) return reply({ sessions: [{ sessionId: "older", cwd: params.cwd }], nextCursor: "page2" });
      return reply({ sessions: [{ sessionId: "s1", cwd: params.cwd }] });
    case "session/set_config_option":
      return reply({ configOptions: params.configId === "mode" ? options(params.value) : options() });
    case "_session/steering":
      return reply({ outcome: cancelTurn ? "injected" : "promptRequired" });
    case "authenticate":
      return reply(params.methodId === "mock-login" ? {} : false);
    case "session/cancel":
      return cancelTurn?.();
    case "session/prompt": {
      const text = params.prompt[0].text;
      const sessionId = params.sessionId;
      if (text === "die") { process.stderr.write("first line\nboom: the mock died\n"); process.exit(3); }
      if (text === "wait") {
        await new Promise((res) => (cancelTurn = res));
        cancelTurn = null;
        return reply({ stopReason: "cancelled" });
      }
      if (text === "tree") {
        const child = Bun.spawn(["sleep", "60"]);
        return reply({ stopReason: `pid ${child.pid}` });
      }
      notify("session/update", { sessionId, update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: `echo: ${text}` }, _meta: { claudeCode: { parentToolUseId: "t0" } } } });
      notify("session/update", { sessionId, update: { sessionUpdate: "current_mode_update", currentModeId: "plan" } });
      notify("_claude/sdkMessage", { sessionId, message: { type: "system", subtype: "mock" } });
      const unknown = await new Promise((res) => { const rid = `m${nextId++}`; waiting.set(rid, res); out({ id: rid, method: "fs/read_text_file", params: { sessionId, path: "/x" } }); });
      const perm = await request("session/request_permission", {
        sessionId, toolCall: { toolCallId: "t1", title: "Run ls" },
        options: [{ optionId: "allow", name: "Allow", kind: "allow_once" }, { optionId: "reject", name: "Reject", kind: "reject_once" }],
      });
      notify("session/update", { sessionId, update: { sessionUpdate: "tool_call_update", toolCallId: "t1", rawOutput: { permission: perm.result, unknown: unknown.error } } });
      return reply({ stopReason: "end_turn" });
    }
    default:
      if (id !== undefined) return error(-32601, `"Method not found": ${method}`);
  }
}

let buf = "";
process.stdin.on("data", (chunk) => {
  buf += chunk.toString();
  let i;
  while ((i = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, i).trim();
    buf = buf.slice(i + 1);
    if (!line) continue;
    const msg = JSON.parse(line);
    if (msg.method === undefined && waiting.has(msg.id)) { const res = waiting.get(msg.id); waiting.delete(msg.id); res(msg); continue; }
    handle(msg);
  }
});
process.stdin.on("end", () => process.exit(0));
