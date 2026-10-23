// The app's MCP tools (R31): served over HTTP at /mcp/app on this loopback server, because an ACP agent runs in its own
// process. Each agent session gets a random secret in the X-Switchboard-Session header; the secret names the TODO (or
// the operator), so every handler still knows its TODO without matching session ids. Stateless: one MCP server per request.
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { WebStandardStreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/webStandardStreamableHttp.js";
import { existsSync, readFileSync, realpathSync, statSync } from "node:fs";
import { basename, isAbsolute, resolve } from "node:path";
import { z } from "zod";
import type { McpServer as McpServerConfig } from "./acp";
import { isClaude } from "./agents";
import { AUTO_EFFORTS, deliveryAction, followUpError, isInside, lineCount, scheduleIn } from "./lib";
import { DEFAULT_STARTED_PER_TURN, commitMessage, isRunning, routeTo, setAutoEffort, start } from "./runner";
// server.ts imports this file too (an ES module cycle): use these only inside functions, never at the top level.
import {
  HOST, PORT, addEvent, addMessage, attachmentsOf, commentsOf, createTodo, db, filesOf, getMeta, liveSessionNo, refreshPrs, removeTodo, sessionsOf, setSchedule, setTodo, summaries, todoRow,
} from "./server";

export const WORKER_TOOLS = ["post_message", "set_title", "set_attention", "attach_file", "set_worktree", "set_pr", "suggest_follow_ups", "list_comments", "reply_comment", "read_todo", "read_file", "create_todo", "remove_todo", "set_schedule", "set_effort"].map((n) => `mcp__app__${n}`);
export const OPERATOR_TOOLS = ["list_todos", "read_todo", "read_file", "create_todo", "send_message", "remove_todo"].map((n) => `mcp__app__${n}`);
// The operator's routing run for one TODO (801): read other TODOs, then decide.
export const ROUTER_TOOLS = ["read_todo", "read_file", "route_todo", "ask_topic"].map((n) => `mcp__app__${n}`);

// `onPost` tells the run that the agent posted a chat message this turn (so its final text is not posted as well).
// `via` names the Claude terminal a grant belongs to (P6): its chat messages carry it.
type Grant = { kind: "worker"; todoId: number; dir: string; onPost: () => void; via?: string } | { kind: "operator" } | { kind: "router"; todoId: number };
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
  if (g.kind === "worker") {
    workerTools(server, g);
    readTools(server, () => {});
    // Item 145: a worker's turn starts at its run; a Claude terminal has no turns in the app.
    // ponytail: a terminal's "turn" is the last 10 minutes; track its prompts if the cap needs to be exact there.
    createTool(server, { parent: g.todoId, used: (d) => addEvent(g.todoId, "create_todo", "agent", [g.via && `from ${g.via}`, d].filter(Boolean).join(" · ")),
      since: () => g.via ? new Date(Date.now() - 600_000).toISOString() : todoRow(g.todoId)!.run_started_at ?? "" });
  }
  else if (g.kind === "router") { readTools(server, (name, detail) => addEvent(g.todoId, name, "operator", detail)); routerTools(server, g.todoId); }
  else {
    const used = (name: string, detail: string | null = null) => addEvent(null, name, "operator", detail);
    readTools(server, used);
    operatorTools(server, used);
    // The operator's turn starts with the user's last message to it.
    createTool(server, { parent: null, used: (d) => used("create_todo", d),
      since: () => (db.query("SELECT committed_at AS at FROM messages WHERE todo_id IS NULL AND author = 'user' ORDER BY id DESC LIMIT 1").get() as { at: string } | null)?.at ?? "" });
  }
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
  s.registerTool("remove_todo", {
    description: "Remove a TODO this TODO created with create_todo, only when the user's instruction says to: like the user's Remove, its run stops and it moves to the Removed fold, where the user can restore it.",
    inputSchema: { todo_id: z.number().int() },
  }, async ({ todo_id }) => {
    const t = todoRow(todo_id);
    if (!t || t.removed_at) return err(`no live TODO T-${todo_id}`);
    if (t.parent_id !== todoId) return err(`T-${todo_id} was not created by this TODO (T-${todoId}); only the user or the operator removes it.`);
    removeTodo(todo_id, `T-${todoId}`);
    addEvent(todoId, "remove_todo", "agent", [g.via && `from ${g.via}`, `T-${todo_id}`].filter(Boolean).join(" · "));
    return ok(`Removed T-${todo_id}. The user can restore it from the Removed fold.`);
  });
  s.registerTool("set_schedule", {
    description: `Set, change, pause or remove (schedule null) the schedule of a TODO this TODO created with create_todo, only when the user's instruction says to. schedule: {days: [1,2,3,4,5], times: ["09:00"]} or {days, every: 30, from: "08:00", to: "18:00"} (days 0 = Sunday, local time), optional prompt (each run's first message; left out, the TODO's first instruction) and paused.`,
    inputSchema: { todo_id: z.number().int(), schedule: z.any() },
  }, async ({ todo_id, schedule }) => {
    const t = todoRow(todo_id);
    if (!t || t.removed_at) return err(`no live TODO T-${todo_id}`);
    if (t.parent_id !== todoId) return err(`T-${todo_id} was not created by this TODO (T-${todoId}); only the user changes its schedule.`);
    const sched = schedule == null ? null : scheduleIn(schedule);
    if (typeof sched === "string") return err(sched);
    setSchedule(todo_id, sched, `T-${todoId}`);
    addEvent(todoId, "set_schedule", "agent", [g.via && `from ${g.via}`, `T-${todo_id}`].filter(Boolean).join(" · "));
    const next = todoRow(todo_id)!.next_run_at;
    return ok(!sched ? `Removed the schedule of T-${todo_id}.` : `T-${todo_id} ${sched.paused ? "is paused" : `runs next at ${next ?? "no slot"}`}.`);
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
    description: "Register the number of the GitHub pull request this TODO works on, in its topic's GitHub repository. The app shows its state, checks and reviews.",
    inputSchema: { number: z.number().int().positive() },
  }, async ({ number }) => {
    setTodo(todoId, { pr_number: number });
    addEvent(todoId, "set_pr", "agent", `#${number}`);
    const pid = todoRow(todoId)!.project_id;
    if (pid) refreshPrs(pid);
    return ok();
  });
  // Auto effort: the worker switches its own effort level as the work changes (CONTRACT's effort rules). Claude only;
  // a Claude terminal does not get it, as the level belongs to the TODO's session.
  if (!g.via && isClaude(todoRow(todoId)!.agent)) s.registerTool("set_effort", {
    description: "Switch your effort level: low, medium, high or xhigh. reason: a few words the user reads, such as \"verifying the fix\". Refused when the user set the effort.",
    inputSchema: { level: z.enum(AUTO_EFFORTS), reason: z.string() },
  }, async ({ level, reason }) => {
    const user = setAutoEffort(todoId, level, reason.trim() || "no reason given", "agent");
    return user ? err(`The user set the effort to ${user}. Keep it.`) : ok(`Effort ${level}.`);
  });
  // Item 146: the user's comments on selected text in the Files tab.
  s.registerTool("list_comments", {
    description: "List the user's open comments on files of this TODO: each has the file's path, the line where the selected text started (when known), the selected text (quote), the comment, and your earlier reply if the user edited the comment after it. Answer each with reply_comment.",
    inputSchema: {},
  }, async () => {
    const open = commentsOf(todoId, "state = 'open'");
    addEvent(todoId, "list_comments", "agent", `${open.length} open`);
    return json(open.map((c) => ({ id: c.id, path: c.path, line: c.line, quote: c.quote, comment: c.body, ...(c.reply ? { your_earlier_reply: c.reply } : {}) })));
  });
  s.registerTool("reply_comment", {
    description: "Reply to one of the user's comments from list_comments. You changed the file for it: start the reply with \"Applied\" and say what you changed in a few words. A question: answer it, no edit. Unclear: ask your question as the reply. The user resolves the comment or edits it to follow up.",
    inputSchema: { id: z.number().int(), reply: z.string() },
  }, async ({ id, reply }) => {
    if (!reply.trim()) return err("reply is empty");
    const c = db.query("SELECT id, path FROM comments WHERE id = ? AND todo_id = ?").get(id, todoId) as { id: number; path: string } | null;
    if (!c) return err(`this TODO has no comment ${id}`);
    db.run("UPDATE comments SET reply = ?, state = 'replied', replied_at = ? WHERE id = ?", [reply.trim(), new Date().toISOString(), id]);
    addEvent(todoId, "reply_comment", "agent", `${basename(c.path)} · ${reply.trim().slice(0, 80)}`);
    return ok();
  });
  // Suggested follow-ups (pills): the TODO's agent only, a terminal has no chat turn to follow up on.
  if (!g.via) s.registerTool("suggest_follow_ups", {
    description: "Suggest one or two follow-ups the user can send you with a click, each one line written as their instruction to you (e.g. \"Approve the PR\"). A later call replaces the earlier ones.",
    inputSchema: { items: z.array(z.string()) },
  }, async ({ items }) => {
    const e = followUpError(items);
    if (e) return err(e);
    const clean = items.map((x) => x.trim());
    setTodo(todoId, { suggestions: JSON.stringify(clean) }, false);
    addEvent(todoId, "suggest", "agent", clean.join(" · "));
    return ok();
  });
}

const latest = (todoId: number, n: number) =>
  db.query("SELECT author, body, committed_at AS at FROM messages WHERE todo_id = ? ORDER BY id DESC LIMIT ?").all(todoId, n).reverse();

type Used = (name: string, detail: string | null) => void;

// A TODO's topic as the agents read it: its name, "none" for a TODO without a topic, null before routing (801).
const topicOf = (s: { project_name: string | null; routed_at: string | null }) => s.project_name ?? (s.routed_at ? "none" : null);

function operatorTools(s: McpServer, used: Used) {
  s.registerTool("list_todos", {
    description: "List the TODOs: number (T-<id>), title, topic (null: not picked yet; \"none\": thinking only), status (not_started = in the backlog, or the Scheduled lane with a schedule), attention, branch, git state, pull request, session count, files, schedule and next run, the TODO that created it, and the latest messages. Removed TODOs only with include_removed.",
    inputSchema: { include_removed: z.boolean().optional() },
  }, async ({ include_removed }) => {
    used("list_todos", null);
    const list = await summaries(include_removed ? "1" : "t.removed_at IS NULL");
    return json(list.map((s) => ({
      id: s.id, ref: `T-${s.id}`, title: s.title, topic: topicOf(s), status: s.status, attention: s.attention, branch: s.branch, git: s.git, pr: s.pr,
      worktree_path: s.worktree_path, sessions: s.session_count, queued_messages: s.queued_count, pending_messages: s.pending_count, files: s.file_names,
      schedule: s.schedule_text && { text: s.schedule_text, ...s.schedule }, next_run_at: s.next_run_at, created_by: s.parent_id ? `T-${s.parent_id}` : null,
      updated_at: s.updated_at, removed_at: s.removed_at, latest_messages: latest(s.id, 3),
    })));
  });
  s.registerTool("send_message", {
    description: "Send a message to a TODO on the user's behalf. It is delivered like a message the user commits.",
    inputSchema: { todo_id: z.number().int(), body: z.string() },
  }, async ({ todo_id, body }) => {
    const t = todoRow(todo_id);
    if (!t || t.removed_at) return err(`no live TODO T-${todo_id}`);
    if (!body.trim()) return err("body is empty");
    const outcome = {
      pending: "stored; it is delivered when the user starts the TODO",
      queue: "queued; the agent gets it when its current turn ends",
      send: "delivered now",
    }[deliveryAction(t.routing ? "not_started" : t.status, isRunning(todo_id))];
    commitMessage(todo_id, "user via operator", body.trim());
    used("send_message", `T-${todo_id}`);
    return ok(`Message ${outcome}.`);
  });
  s.registerTool("remove_todo", {
    description: "Remove a TODO, only when the user asks: like the user's Remove, its run stops and it moves to the Removed fold, where the user can restore it.",
    inputSchema: { todo_id: z.number().int() },
  }, async ({ todo_id }) => {
    const t = todoRow(todo_id);
    if (!t || t.removed_at) return err(`no live TODO T-${todo_id}`);
    removeTodo(todo_id, "operator");
    used("remove_todo", `T-${todo_id}`);
    return ok(`Removed T-${todo_id}. The user can restore it from the Removed fold.`);
  });
}

// Item 145: create_todo for the operator (`parent` null), the workers and Claude terminals.
// `used` records the call, with "· started" for the cap; `since` is when the caller's current turn began.
function createTool(s: McpServer, by: { parent: number | null; used: (detail: string) => void; since: () => string }) {
  const op = by.parent === null;
  const topics = db.query("SELECT id, name FROM projects ORDER BY name").all() as { id: number; name: string }[];
  const names = `${topics.map((x) => x.name).join(", ") || "(none yet)"}, or "none"`;
  s.registerTool("create_todo", {
    description: `Create a TODO with its first instruction. title: a short title for its card; leave it out and the TODO's agent sets one. topic: one of ${names} for a thinking-only TODO; leave it out and routing picks it at the start. start: true starts it now, only when the user's words say to start it; otherwise it lands in the user's backlog. key: a dedupe key such as "pr:copilot-service#490"; when a TODO has that key already, removed ones too, nothing is created.`
      + ` schedule: {days: [1,2,3,4,5], times: ["09:00"]} or {days, every: 30, from: "08:00", to: "18:00"} (days 0 = Sunday, local time) makes a scheduled TODO that runs a new session at each slot; only when the user asks for one.`,
    inputSchema: { instruction: z.string(), title: z.string().optional(), topic: z.string().optional(), start: z.boolean().optional(), key: z.string().optional(), schedule: z.any().optional() },
  }, async ({ instruction, title, topic, start: go, key, schedule }: { instruction: string; title?: string; topic?: string; start?: boolean; key?: string; schedule?: unknown }) => {
    if (!instruction.trim()) return err("instruction is empty");
    let project: number | null | undefined;
    if (topic !== undefined) {
      const name = topic.trim().toLowerCase(), p = topics.find((x) => x.name.toLowerCase() === name);
      if (name !== "none" && !p) return err(`Unknown topic "${topic}". Topics: ${names}.`);
      project = p?.id ?? null;
    }
    const sched = schedule == null ? null : scheduleIn(schedule);
    if (typeof sched === "string") return err(sched);
    const k = key?.trim() || null;
    const had = k && (db.query("SELECT id, removed_at FROM todos WHERE key = ?").get(k) as { id: number; removed_at: string | null } | null);
    if (had) {
      by.used(`not created · T-${had.id} has the key`);
      return ok(`Not created: T-${had.id} has key ${k}${had.removed_at ? ` (removed ${had.removed_at.slice(0, 10)})` : ""}.`);
    }
    const cap = Number(getMeta("started_todos_per_turn")) || DEFAULT_STARTED_PER_TURN;
    const started = (db.query("SELECT COUNT(*) AS n FROM events WHERE todo_id IS ? AND kind = 'create_todo' AND detail LIKE '%· started%' AND at >= ?").get(by.parent, by.since()) as { n: number }).n;
    if (go && started >= cap) return err(`Cap of ${cap} started TODOs per turn reached. Create the rest with start: false, or the user raises it in Settings › Runs.`);
    const id = createTodo(instruction.trim(), op ? "operator" : "agent", [], null, { parent_id: by.parent, key: k, project_id: project, schedule: sched, title: title?.trim() || null });
    if (go) start(id);
    const name = project === undefined ? null : topics.find((x) => x.id === project)?.name ?? "none";
    by.used([`T-${id}`, go && "started", sched && "scheduled", name && `topic ${name}`].filter(Boolean).join(" · "));
    return ok(`Created T-${id} · ${go ? "started" : sched ? "scheduled" : "in the backlog"}${name ? ` · topic ${name}.` : go ? "; routing picks its topic." : "; routing picks its topic when it starts."}`);
  });
}

// read_todo and read_file, the same for the operator, its routing runs and the workers (801). `used` logs a call.
function readTools(s: McpServer, used: Used) {
  s.registerTool("read_todo", {
    description: "Read one TODO by its number (T-42 is 42), removed ones too: its details, its sessions with their chat messages, and its file list.",
    inputSchema: { todo_id: z.number().int() },
  }, async ({ todo_id }) => {
    used("read_todo", `T-${todo_id}`);
    const [s] = await summaries("t.id = ?", [todo_id]);
    if (!s) return err(`no TODO T-${todo_id}`);
    const messages = (db.query(`SELECT m.id, m.author, m.body, m.state, m.committed_at AS at, m.session_no, f.path AS file
      FROM messages m LEFT JOIN files f ON f.id = m.file_id WHERE m.todo_id = ? ORDER BY m.id`).all(todo_id) as any[])
      // the user's uploads as absolute paths, for Read (an image opens as an image)
      .map(({ id, ...m }) => {
        const paths = attachmentsOf(id).map((a) => a.path);
        return paths.length ? { ...m, attachments: paths } : m;
      });
    const sessions = sessionsOf(todo_id);
    const live = sessions.at(-1)?.no ?? null;
    const label = (no: number) => (messages.find((m) => m.session_no === no && m.author.startsWith("user"))?.body ?? "").split("\n")[0];
    return json({
      id: s.id, ref: `T-${s.id}`, title: s.title, topic: topicOf(s), status: s.status, attention: s.attention, branch: s.branch, git: s.git, pr: s.pr,
      pr_checked_at: s.pr_checked_at, gh_error: s.gh_error, worktree_path: s.worktree_path, live_session_id: s.session_id, created_at: s.created_at,
      removed_at: s.removed_at, removed_by: s.removed_by,
      schedule: s.schedule_text && { text: s.schedule_text, ...s.schedule }, next_run_at: s.next_run_at, created_by: s.parent_id ? `T-${s.parent_id}` : null, key: s.key,
      files: filesOf(todo_id).map((f) => ({ name: basename(f.path), path: f.path, session: f.session_no })),
      sessions: sessions.map((x) => ({ ...x, label: label(x.no), messages: messages.filter((m) => m.session_no === x.no) })),
      // pending (before the start) and queued messages wait for the live session
      waiting_messages: messages.filter((m) => m.session_no == null && m.state !== "delivered").map((m) => ({ ...m, for_session: live })),
    });
  });
  s.registerTool("read_file", {
    description: "Read one of a TODO's text files by its name: its attached markdown files and the user's uploads. For an image or other binary file it returns the path to open with Read.",
    inputSchema: { todo_id: z.number().int(), name: z.string() },
  }, async ({ todo_id, name }) => {
    used("read_file", `T-${todo_id} ${name}`);
    const uploads = db.query("SELECT a.name, a.path FROM attachments a JOIN messages m ON m.id = a.message_id WHERE m.todo_id = ? ORDER BY a.id")
      .all(todo_id) as { name: string; path: string }[];
    const path = filesOf(todo_id).find((x) => basename(x.path) === name)?.path
      ?? uploads.find((a) => basename(a.path) === name || a.name === name)?.path;
    if (!path) return err(`T-${todo_id} has no file named ${name}`);
    if (!existsSync(path)) return err(`${path} no longer exists on disk`);
    const data = readFileSync(path);
    // ponytail: a NUL byte means binary (every PNG, JPEG, PDF has one); UTF-16 text would count as binary too
    if (data.includes(0)) return ok(`${path} is not a text file. Open it with Read.`);
    return ok(data.toString("utf8"));
  });
}

// The routing run's decision (801). The handlers only record it; runner.ts starts session 1 once the run ends.
function routerTools(s: McpServer, todoId: number) {
  const topics = () => db.query("SELECT id, name FROM projects ORDER BY name").all() as { id: number; name: string }[];
  const find = (name: string) => topics().find((x) => x.name === name) ?? topics().find((x) => x.name.toLowerCase() === name.trim().toLowerCase());
  const names = () => topics().map((x) => x.name).join(", ") || "none";
  const decided = () => !!todoRow(todoId)!.routed_at || JSON.parse(todoRow(todoId)!.routing ?? "{}").state === "asking";
  s.registerTool("route_todo", {
    description: "Decide which topic this TODO belongs to: a topic's name, or \"none\" for a thinking-only TODO without a topic. The reason is one short sentence the user reads. effort: the level the TODO's agent starts at (low, medium, high, xhigh); leave it out for medium. Stop after this call.",
    inputSchema: { topic: z.string(), reason: z.string(), effort: z.enum(AUTO_EFFORTS).optional() },
  }, async ({ topic, reason, effort }) => {
    if (decided()) return err("This TODO's routing is decided already. Stop now.");
    const none = topic.trim().toLowerCase() === "none";
    const p = none ? null : find(topic);
    if (!none && !p) return err(`Unknown topic "${topic}". Topics: ${names()}, or "none".`);
    addEvent(todoId, "route_todo", "operator", `topic: "${p?.name ?? "none"}"`);
    routeTo(todoId, p?.id ?? null, reason);
    if (effort && isClaude(todoRow(todoId)!.agent)) setAutoEffort(todoId, effort, "picked at the start", "operator");
    return ok(`Routed to ${p?.name ?? "no topic"}. The app starts session 1 there when you stop.`);
  });
  s.registerTool("ask_topic", {
    description: "Ask the user which topic this TODO belongs to, when you cannot tell: a short question and the 2 or 3 likely topics by name. The user also gets a \"No topic\" choice. Stop after this call.",
    inputSchema: { question: z.string(), options: z.array(z.string()) },
  }, async ({ question, options }) => {
    if (decided()) return err("This TODO's routing is decided already. Stop now.");
    if (!question.trim()) return err("question is empty");
    const picked = options.map(find);
    if (picked.some((p) => !p)) return err(`Unknown topic in ${JSON.stringify(options)}. Topics: ${names()}.`);
    const opts = [...new Map(picked.map((p) => [p!.id, { id: p!.id, name: p!.name }])).values()];
    if (opts.length < 1 || opts.length > 3) return err("Give one to three topics.");
    addEvent(todoId, "ask_topic", "operator", opts.map((o) => o.name).join(", "));
    addMessage(todoId, "operator", question.trim());
    setTodo(todoId, { routing: JSON.stringify({ state: "asking", question: question.trim(), options: opts }) });
    return ok("Asked. The user's click or reply decides; stop now.");
  });
}
