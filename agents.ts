// The agent registry (P5): which ACP agents a TODO can run, and what differs between them. For now Claude is the only
// built-in (Codex and Gemini wait until they can be tested); the user's own agents would be rows of the `agents` table,
// whose Settings page stays hidden while Claude is the only agent. runner.ts asks this file how to start an agent
// and how to talk to it; acp.ts knows nothing about agents beyond the command line it is given.
import { dirname, join } from "node:path";
import type { Spec } from "./acp";
// server.ts imports this file too (an ES module cycle): use these only inside functions, never at the top level.
import { db } from "./server";

export type AgentRow = { id: string; name: string; command: string; args: string; env: string; created_at: string };
export type AgentInfo = {
  id: string; name: string; builtin: boolean;
  command: string;        // as shown in Settings
  found: boolean;         // the command can be started
  hint: string | null;    // how to get it when it is not found
};

const adapter = (pkg: string) => join(dirname(Bun.resolveSync(`${pkg}/package.json`, import.meta.dir)), "dist/index.js");

// The bundled adapter runs on the Bun that runs this server (verified in the spike): no Node needed. Codex's adapter,
// @agentclientprotocol/codex-acp, ran on Bun too (P5 probe), so it would be `adapter("@agentclientprotocol/codex-acp")`.
const BUILTINS: { id: string; name: string; command: () => string[] | null; shown: string; hint: string | null }[] = [
  { id: "claude", name: "Claude", command: () => [process.execPath, adapter("@agentclientprotocol/claude-agent-acp")], shown: "claude-agent-acp (bundled)", hint: null },
];
export const DEFAULT_AGENT = "claude";

// A command found on PATH, with its arguments. A Windows .cmd or .bat shim (npm's global installs) runs through cmd.exe.
export function onPath(cmd: string, args: string[]): string[] | null {
  const path = Bun.which(cmd);
  if (!path) return null;
  return process.platform === "win32" && /\.(cmd|bat)$/i.test(path) ? ["cmd.exe", "/d", "/s", "/c", path, ...args] : [path, ...args];
}

const customRow = (id: string) => db.query("SELECT * FROM agents WHERE id = ?").get(id) as AgentRow | null;
export const customRows = () => db.query("SELECT * FROM agents ORDER BY name").all() as AgentRow[];

// How to start an agent; null when its command is not there (not installed, or an id no longer in the registry).
export function specOf(id: string): Spec | null {
  const b = BUILTINS.find((x) => x.id === id);
  if (b) {
    const command = b.command();
    return command && { command };
  }
  const r = customRow(id);
  if (!r) return null;
  const command = onPath(r.command, JSON.parse(r.args || "[]"));
  return command && { command, env: JSON.parse(r.env || "{}") };
}

// Every agent, built-ins first, for Settings › Agents and the agent selects.
export function agentList(): AgentInfo[] {
  return [
    ...BUILTINS.map((b) => ({ id: b.id, name: b.name, builtin: true, command: b.shown, found: !!b.command(), hint: b.hint })),
    ...customRows().map((r) => ({
      id: r.id, name: r.name, builtin: false, command: [r.command, ...JSON.parse(r.args || "[]")].join(" "),
      found: !!Bun.which(r.command), hint: null,
    })),
  ];
}

export const agentName = (id: string | null) => agentList().find((a) => a.id === (id || DEFAULT_AGENT))?.name ?? id ?? "Claude";
export const isBuiltin = (id: string) => BUILTINS.some((b) => b.id === id);
export const isKnown = (id: string) => isBuiltin(id) || !!customRow(id);

// Claude gets the contract as a system-prompt append and its options through the adapter's `_meta`; every other agent
// gets the contract as text in front of the first prompt of each session (design §9).
export const isClaude = (id: string | null) => (id || DEFAULT_AGENT) === "claude";

// How the contract names an app tool for this agent: Claude Code's own MCP tool name, else a description any agent
// maps to its own naming (Codex, for one, does not use the mcp__ prefix).
export const toolName = (agent: string | null, tool: string) => isClaude(agent) ? `mcp__app__${tool}` : `the app MCP server's ${tool} tool`;

// The command that resumes a session outside the app, where the agent has one (item 78).
export function resumeHint(agent: string | null, sessionId: string): string | null {
  const id = agent || DEFAULT_AGENT;
  if (id === "claude") return `claude --resume ${sessionId}`;
  return null;
}

// The MCP secret header: Claude Code expands ${VAR} from the agent process's environment, so the secret stays out of
// the CLI's argv (readable by every user in /proc). Other agents get it as is: they take the MCP config over ACP.
export const secretHeader = (agent: string | null, secret: string) => isClaude(agent) ? "${SWITCHBOARD_MCP_SECRET}" : secret;
