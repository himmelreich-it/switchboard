// The app's MCP tools (R31): served over HTTP at /mcp/app on this loopback server, because an ACP agent runs in its own
// process. Each agent session gets a random secret in the X-Switchboard-Session header; the secret names the TODO (or
// the operator), so every handler still knows its TODO without matching session ids. Stateless: one MCP server per request.
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { WebStandardStreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/webStandardStreamableHttp.js";
import { existsSync, readFileSync, realpathSync, statSync } from "node:fs";
import { basename, isAbsolute, resolve } from "node:path";
import { z } from "zod";
import type { McpServer as McpServerConfig } from "./acp";
import { deliveryAction, isInside, lineCount } from "./lib";
import { commitMessage, isRunning } from "./runner";
// server.ts imports this file too (an ES module cycle): use these only inside functions, never at the top level.
import {
  HOST, PORT, addEvent, addMessage, createTodo, db, filesOf, liveSessionNo, refreshPrs, sessionsOf, setTodo, summaries, todoRow,
} from "./server";

export const WORKER_TOOLS = ["post_message", "set_title", "set_attention", "attach_file", "set_worktree", "set_pr"].map((n) => `mcp__app__${n}`);
export const OPERATOR_TOOLS = ["list_todos", "read_todo", "read_file", "create_todo", "send_message"].map((n) => `mcp__app__${n}`);

// `onPost` tells the run that the agent posted a chat message this turn (so its final text is not posted as well).
// `via` names the Claude terminal a grant belongs to (P6): its chat messages carry it.
type Grant = { kind: "worker"; todoId: number; dir: string; onPost: () => void; via?: string } | { kind: "operator" };
const grants = new Map<string, Grant>();

// A secret for one agent process, and the MCP server entry that names it. Revoke it when the agent process ends.
// `header` gives the header value for the secret: Claude's is a ${VAR} reference to the agent process's environment
// (runner.ts sets SWITCHBOARD_MCP_SECRET), see agents.ts secretHeader.
export function grant(g: Grant, header: (secret: string) => string): { secret: string; server: McpServerConfig } {
  const secret = Buffer.from(crypto.getRandomValues(new Uint8Array(32))).toString("hex");
  grants.set(secret, g);
  return {
    secret,
    server: { type: "http", name: "app", url: `http://${HOST}:${PORT}/mcp/app`, headers: [{ name: "X-Switchboard-Session", value: header(secret) }] } as McpServerConfig,
  };
}
export const revoke = (secret: string) => grants.delete(secret);

export async function handleMcp(req: Request): Promise<Response> {
  const g = grants.get(req.headers.get("x-switchboard-session") ?? "");
  if (!g) return new Response("unknown session", { status: 403 });
  // Stateless: no server-initiated stream (GET) and no session to end (DELETE).
  if (req.method !== "POST") return new Response(null, { status: 405 });
  const server = new McpServer({ name: "app", version: "1.0.0" });
  if (g.kind === "worker") workerTools(server, g);
  else operatorTools(server);
  const transport = new WebStandardStreamableHTTPServerTransport({ sessionIdGenerator: undefined, enableJsonResponse: true });
  await server.connect(transport);
  return transport.handleRequest(req);
}

const ok = (text = "ok") => ({ content: [{ type: "text" as const, text }] });
const err = (text: string) => ({ content: [{ type: "text" as const, text }], isError: true });
const json = (v: unknown) => ok(JSON.stringify(v, null, 1));

function workerTools(s: McpServer, g: Extract<Grant, { kind: "worker" }>) {
  const { todoId, dir } = g;
  s.registerTool("post_message", {
    description: "Post a one or two line chat message to the user. Pass file_name to link a file you registered with attach_file.",
    inputSchema: { body: z.string(), file_name: z.string().optional() },
  }, async ({ body, file_name }) => {
    if (!body.trim()) return err("body is empty");
    if (lineCount(body) > 2) return err(`Chat messages are one or two lines. Put the longer content in a markdown file in ${dir}, register it with attach_file, and link it with file_name.`);
    let fileId: number | null = null;
    if (file_name) {
      const f = filesOf(todoId).find((x) => basename(x.path) === basename(file_name));
      if (!f) return err(`No attached file is named ${file_name}. Call attach_file first.`);
      fileId = f.id;
    }
    addMessage(todoId, "agent", body.trim(), "delivered", fileId, g.via ?? null);
    addEvent(todoId, "post_message", "agent", [g.via && `from ${g.via}`, file_name].filter(Boolean).join(" · ") || null);
    g.onPost();
    return ok();
  });
  s.registerTool("set_title", { description: "Set this TODO's title: a short summary of the task.", inputSchema: { title: z.string() } }, async ({ title }) => {
    if (!title.trim()) return err("title is empty");
    setTodo(todoId, { title: title.trim() });
    addEvent(todoId, "set_title", "agent", title.trim());
    return ok();
  });
  s.registerTool("set_attention", {
    description: "Raise the attention flag (raised: true) when you need the user; clear it (raised: false) once their reply resolves it.",
    inputSchema: { raised: z.boolean() },
  }, async ({ raised }) => {
    setTodo(todoId, { attention: raised ? 1 : 0, attention_by: raised ? "agent" : null });
    addEvent(todoId, raised ? "attention raised" : "attention cleared", "agent");
    return ok();
  });
  s.registerTool("attach_file", {
    description: `Register a file you wrote in this TODO's artifact folder (${dir}) so the user can open it. Pass its absolute path.`,
    inputSchema: { path: z.string() },
  }, async ({ path }) => {
    if (!isAbsolute(path)) return err("path must be absolute");
    if (!existsSync(path)) return err(`${path} does not exist`);
    const real = realpathSync(path); // so a symlink cannot point the user's editor outside the folder
    if (!isInside(real, realpathSync(dir))) return err(`${path} is outside this TODO's artifact folder ${dir}`);
    if (!statSync(real).isFile()) return err(`${path} is not a file`);
    // A file found on disk earlier has no session yet; registering it claims it for this one.
    db.run("INSERT INTO files (todo_id, path, session_no) VALUES (?, ?, ?) ON CONFLICT(path) DO UPDATE SET session_no = COALESCE(session_no, excluded.session_no)",
      [todoId, real, liveSessionNo(todoId)]);
    addEvent(todoId, "attach_file", "agent", basename(real));
    return ok(`attached as ${basename(real)}`);
  });
  s.registerTool("set_worktree", { description: "Register the absolute path of the git worktree this TODO works in.", inputSchema: { path: z.string() } }, async ({ path }) => {
    if (!isAbsolute(path) || !existsSync(path) || !statSync(path).isDirectory()) return err(`${path} is not an existing directory (absolute path required)`);
    setTodo(todoId, { worktree_path: resolve(path) });
    addEvent(todoId, "set_worktree", "agent", resolve(path));
    return ok();
  });
  s.registerTool("set_pr", {
    description: "Register the number of the GitHub pull request this TODO works on, in the project's repository. The app shows its state, checks and reviews.",
    inputSchema: { number: z.number().int().positive() },
  }, async ({ number }) => {
    setTodo(todoId, { pr_number: number });
    addEvent(todoId, "set_pr", "agent", `#${number}`);
    refreshPrs(todoRow(todoId)!.project_id);
    return ok();
  });
}

const latest = (todoId: number, n: number) =>
  db.query("SELECT author, body, committed_at AS at FROM messages WHERE todo_id = ? ORDER BY id DESC LIMIT ?").all(todoId, n).reverse();

function operatorTools(s: McpServer) {
  const used = (name: string, detail: string | null = null) => addEvent(null, name, "operator", detail);
  s.registerTool("list_todos", {
    description: "List the TODOs: title, project, status (not_started = in the backlog), attention, branch, git state, pull request, session count, files and the latest messages. Removed TODOs only with include_removed.",
    inputSchema: { include_removed: z.boolean().optional() },
  }, async ({ include_removed }) => {
    used("list_todos");
    const list = await summaries(include_removed ? "1" : "t.removed_at IS NULL");
    return json(list.map((s) => ({
      id: s.id, title: s.title, project: s.project_name, status: s.status, attention: s.attention, branch: s.branch, git: s.git, pr: s.pr,
      worktree_path: s.worktree_path, sessions: s.session_count, queued_messages: s.queued_count, pending_messages: s.pending_count, files: s.file_names,
      updated_at: s.updated_at, removed_at: s.removed_at, latest_messages: latest(s.id, 3),
    })));
  });
  s.registerTool("read_todo", {
    description: "Read one TODO (removed ones too): its details, its sessions with their chat messages, and its file list.",
    inputSchema: { todo_id: z.number().int() },
  }, async ({ todo_id }) => {
    used("read_todo", `#${todo_id}`);
    const [s] = await summaries("t.id = ?", [todo_id]);
    if (!s) return err(`no TODO #${todo_id}`);
    const messages = db.query(`SELECT m.author, m.body, m.state, m.committed_at AS at, m.session_no, f.path AS file
      FROM messages m LEFT JOIN files f ON f.id = m.file_id WHERE m.todo_id = ? ORDER BY m.id`).all(todo_id) as any[];
    const sessions = sessionsOf(todo_id);
    const live = sessions.at(-1)?.no ?? null;
    const label = (no: number) => (messages.find((m) => m.session_no === no && m.author.startsWith("user"))?.body ?? "").split("\n")[0];
    return json({
      id: s.id, title: s.title, project: s.project_name, status: s.status, attention: s.attention, branch: s.branch, git: s.git, pr: s.pr,
      pr_checked_at: s.pr_checked_at, gh_error: s.gh_error, worktree_path: s.worktree_path, live_session_id: s.session_id, created_at: s.created_at,
      removed_at: s.removed_at, removed_by: s.removed_by,
      files: filesOf(todo_id).map((f) => ({ name: basename(f.path), path: f.path, session: f.session_no })),
      sessions: sessions.map((x) => ({ ...x, label: label(x.no), messages: messages.filter((m) => m.session_no === x.no) })),
      // pending (before the start) and queued messages wait for the live session
      waiting_messages: messages.filter((m) => m.session_no == null && m.state !== "delivered").map((m) => ({ ...m, for_session: live })),
    });
  });
  s.registerTool("read_file", {
    description: "Read one of a TODO's attached markdown files by its name.",
    inputSchema: { todo_id: z.number().int(), name: z.string() },
  }, async ({ todo_id, name }) => {
    used("read_file", `#${todo_id} ${name}`);
    const f = filesOf(todo_id).find((x) => basename(x.path) === name);
    if (!f) return err(`TODO #${todo_id} has no file named ${name}`);
    if (!existsSync(f.path)) return err(`${f.path} no longer exists on disk`);
    return ok(readFileSync(f.path, "utf8"));
  });
  s.registerTool("create_todo", {
    description: "Create a TODO in the named project with its first instruction. It stays not started; the user starts it.",
    inputSchema: { project: z.string(), instruction: z.string() },
  }, async ({ project, instruction }) => {
    const all = db.query("SELECT id, name FROM projects").all() as { id: number; name: string }[];
    const p = all.find((x) => x.name === project) ?? all.find((x) => x.name.toLowerCase() === project.trim().toLowerCase());
    if (!p) return err(`Unknown project "${project}". Projects: ${all.map((x) => x.name).join(", ") || "none"}`);
    if (!instruction.trim()) return err("instruction is empty");
    const id = createTodo(p.id, instruction.trim(), "operator");
    used("create_todo", `#${id} in ${p.name}`);
    return ok(`Created TODO #${id} in ${p.name}. It is in the backlog until the user starts it.`);
  });
  s.registerTool("send_message", {
    description: "Send a message to a TODO on the user's behalf. It is delivered like a message the user commits.",
    inputSchema: { todo_id: z.number().int(), body: z.string() },
  }, async ({ todo_id, body }) => {
    const t = todoRow(todo_id);
    if (!t || t.removed_at) return err(`no live TODO #${todo_id}`);
    if (!body.trim()) return err("body is empty");
    const outcome = {
      pending: "stored; it is delivered when the user starts the TODO",
      queue: "queued; the agent gets it when its current turn ends",
      send: "delivered now",
    }[deliveryAction(t.status, isRunning(todo_id))];
    commitMessage(todo_id, "user via operator", body.trim());
    used("send_message", `#${todo_id}`);
    return ok(`Message ${outcome}.`);
  });
}
