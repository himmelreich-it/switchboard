// Agent SDK runs: one query() per turn for each TODO's worker, plus the operator.
import { createSdkMcpServer, query, tool, type Options, type Query } from "@anthropic-ai/claude-agent-sdk";
import { existsSync, mkdirSync, readFileSync, realpathSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { basename, isAbsolute, resolve } from "node:path";
import { z } from "zod";
import {
  cutToTwoLines, deliveryAction, deliveryPrompt, handOver, isInside, lineCount, statusAfterRun, toolArg, toolLine, usageStep, withAttachments, type Ctx,
} from "./lib";
// server.ts imports this file too (an ES module cycle): use these only inside functions, never at the top level.
import {
  HOME, addEvent, addMessage, artifactDir, attachmentsOf, createTodo, db, emit, filesOf, getMeta, liveSessionNo, now, projectRow, refreshPrs,
  saveAttachments, sessionsOf, setMeta, setTodo, summaries, todoRow, type Upload,
} from "./server";

// The fixed app contract (R31), appended to the claude_code system prompt before the project prompt.
// GET /api/meta shows it with the placeholder; each run fills in the TODO's own folder.
export const CONTRACT = `You are running inside switchboard, a local app where the user tracks this work as a TODO and steers it with short chat messages. A TODO can span several Claude Code sessions, one after another; this session is one of them.

- Talk to the user only through the app's MCP tools: mcp__app__post_message, mcp__app__set_title, mcp__app__set_attention, mcp__app__attach_file, mcp__app__set_worktree and mcp__app__set_pr. On your first turn, call mcp__app__set_title with a short title for this TODO, unless the prompt starts with a hand-over header.
- A prompt that starts with "This TODO continues earlier work" is a hand-over from earlier sessions. Keep the title it names, work in the worktree it names instead of creating a new one, and read the listed files you need before you start.
- Keep chat messages (mcp__app__post_message) to one or two lines. Anything longer (plans, reviews, questions with context, designs) goes in a markdown file in your artifact folder {artifact_dir}. Register each file with mcp__app__attach_file and link it from the message with file_name.
- The TODO text and the user's committed messages are your go-ahead for local edits within this TODO's scope. Outward actions (push, PR comments, Jira, Slack) still need an explicit message from the user.
- Work out from the TODO text, and from any referenced ticket, what kind of work this is (a review, a feature, or thinking only with no code changes) and whether it needs a worktree. Follow the project prompt for how to create one.
- After creating a worktree, call mcp__app__set_worktree with its absolute path and work only there. Ignore any skill instruction not to switch context.
- As soon as you know this TODO works on an existing GitHub pull request (a review, a merge check, a fix on someone's PR), call mcp__app__set_pr with its number, also without a worktree. The app then shows the PR's state, checks and reviews on the TODO's card. A PR opened from your worktree's branch is found without it.
- When anything is unclear, ask rather than guess: post a message and raise attention with mcp__app__set_attention.
- When auto mode blocks an action, or you are unsure how to proceed, post a message and raise attention instead of routing around the block.
- Clear your attention flag (mcp__app__set_attention with raised false) once the user's reply resolves it.`;

const OPERATOR_PROMPT = (home: string) => `You are the operator of switchboard, a local app where each TODO is a Claude Code session the user starts and steers. You are the user's chief of staff for those sessions, not their manager.

- Act only on the user's explicit instruction in this chat. Never poll, never answer a worker's question on your own, and never start TODOs: the user starts them.
- Get the state of the TODOs from your tools every time (mcp__app__list_todos, mcp__app__read_todo, mcp__app__read_file), not from earlier turns of this chat: those go stale, and the user can start a fresh operator session at any time.
- A TODO with status not_started is in the user's backlog; the others are on the board. A TODO can span several sessions; only the latest is live.
- mcp__app__create_todo creates a TODO in the backlog (not started). mcp__app__send_message sends a message to a TODO on the user's behalf; it shows in that TODO's chat as "via operator". Use them only when the user asks.
- TODO artifact files live under ${home}/data/<project-slug>/<todo-id>/. You can also read them with Read, Grep and Glob.
- Your replies are shown in full and rendered as markdown. Brainstorming with the user here is fine.`;

const WORKER_TOOLS = ["post_message", "set_title", "set_attention", "attach_file", "set_worktree", "set_pr"].map((n) => `mcp__app__${n}`);
const OPERATOR_TOOLS = ["list_todos", "read_todo", "read_file", "create_todo", "send_message"].map((n) => `mcp__app__${n}`);
const OPERATOR_BUILTINS = ["Read", "Grep", "Glob", "WebSearch", "WebFetch"];

type Key = number | "operator"; // a TODO id, or the operator
type Run = { q: Query; abort: AbortController; done: Promise<void> };
const running = new Map<Key, Run>();
const interrupting = new Set<Key>(); // runs the app stopped itself (remove, shutdown): no error message for those
let shuttingDown = false;

export const isRunning = (key: Key) => running.has(key);

const ok = (text = "ok") => ({ content: [{ type: "text" as const, text }] });
const err = (text: string) => ({ content: [{ type: "text" as const, text }], isError: true });

// Like Esc in Claude Code. If the CLI cannot take the interrupt, or ignores it, the process is killed.
export function interrupt(key: Key) {
  const r = running.get(key);
  if (!r) return;
  interrupting.add(key);
  r.q.interrupt().catch(() => r.abort.abort());
  setTimeout(() => running.get(key) === r && r.abort.abort(), 10_000);
}

export async function stopAll() {
  shuttingDown = true;
  const runs = [...running.values()];
  for (const key of running.keys()) interrupt(key);
  await Promise.allSettled(runs.map((r) => r.done));
}

// Marks the messages delivered to the live session and returns their bodies with the paths of their attachments, oldest first.
function take(todoId: number | null, state: "pending" | "queued") {
  const msgs = db.query("SELECT id, body FROM messages WHERE todo_id IS ? AND state = ? ORDER BY id").all(todoId, state) as { id: number; body: string }[];
  const at = now(), no = liveSessionNo(todoId);
  for (const m of msgs) db.run("UPDATE messages SET state = 'delivered', delivered_at = ?, session_no = ? WHERE id = ?", [at, no, m.id]);
  return msgs.map((m) => withAttachments(m.body, attachmentsOf(m.id).map((a) => a.path)));
}

// Runs the for-await loop of one query and reports how it ended. A thrown loop is how interrupt() and process
// exits show up; the first error wins, so a failed result followed by the exit is reported once.
async function drain(q: Query, onMessage: (m: any) => void) {
  let text = "";
  let error = "";
  try {
    for await (const m of q) {
      onMessage(m);
      if (m.type !== "result") continue;
      if (m.subtype === "success" && !m.is_error) text = m.result;
      else error ||= m.subtype === "success" ? m.result : `${m.subtype}: ${m.errors.join("; ")}`;
    }
  } catch (e: any) {
    error ||= e?.message ?? String(e);
  }
  return { text, error };
}

// ---------- workers ----------

// A committed message: stored until start, queued while running, or sent now (R20). The attachments are saved in the
// same transaction and before deliver(), whose take() reads them.
export function commitMessage(todoId: number, author: "user" | "user via operator", body: string, uploads: Upload[] = []) {
  const action = deliveryAction(todoRow(todoId)!.status);
  const id = db.transaction(() => {
    const id = addMessage(todoId, author, body, action === "pending" ? "pending" : "queued");
    saveAttachments(todoId, id, uploads);
    return id;
  })();
  if (action === "send") deliver(todoId);
  return id;
}

// Opens session `no` of a TODO (item 68): the sessions row, todos.session_id (what delivery and restore resume), and its
// first run under a session id made here. Every session starts here: Start for session 1, New session for the rest.
// `before` runs once the new session is live, so what it records belongs to that session.
function openSession(todoId: number, no: number, takeBodies: () => string[], reason: string, header = "", before?: (sessionId: string) => void) {
  const sessionId = crypto.randomUUID();
  db.run("INSERT INTO sessions (todo_id, no, session_id, started_at) VALUES (?, ?, ?, ?)", [todoId, no, sessionId, now()]);
  setTodo(todoId, { session_id: sessionId, ctx: null });
  before?.(sessionId);
  runWorker(todoId, takeBodies, { sessionId }, reason, header);
}

// The first turn: every message committed before the start goes out as one prompt.
export function start(todoId: number) {
  openSession(todoId, 1, () => take(todoId, "pending"), "start");
}

// Item 72: ends the live session (it stays as history), clears attention, and starts the next session with the
// hand-over header plus every queued message, because the ended session gets no more runs.
export function newSession(todoId: number) {
  const t = todoRow(todoId)!;
  const prev = sessionsOf(todoId).at(-1)!;
  const no = prev.no + 1;
  db.run("UPDATE sessions SET ended_at = ? WHERE todo_id = ? AND no = ?", [now(), todoId, prev.no]);
  if (t.attention) {
    setTodo(todoId, { attention: 0, status: t.status === "waiting" ? "idle" : t.status });
    addEvent(todoId, "attention cleared", "user");
  }
  const last = db.query("SELECT body, session_no FROM messages WHERE todo_id = ? AND author = 'agent' ORDER BY id DESC LIMIT 1").get(todoId) as any;
  const header = handOver({
    no, title: t.title, worktree: t.worktree_path, pr: t.pr_number,
    last: last ? { no: last.session_no ?? prev.no, body: last.body } : null,
    files: filesOf(todoId).map((f) => ({ path: f.path, session_no: f.session_no })),
  });
  openSession(todoId, no, () => take(todoId, "queued"), "new session", header,
    (sid) => addEvent(todoId, "new session", "user", `session ${no} · ${sid}\n${header}`));
}

// Resumes the session with every queued message as one prompt. Does nothing while the TODO runs (the run's end calls
// this again), when it is removed or not started, or when nothing is queued.
export function deliver(todoId: number, reason = "deliver") {
  const t = todoRow(todoId);
  if (running.has(todoId) || !t?.session_id || t.removed_at || t.status === "not_started") return;
  const queued = (db.query("SELECT COUNT(*) AS n FROM messages WHERE todo_id = ? AND state = 'queued'").get(todoId) as { n: number }).n;
  if (queued) runWorker(todoId, () => take(todoId, "queued"), { resume: t.session_id }, reason);
}

// `git config worktree.root`, else <local_path>/../worktrees. Created if missing, so --add-dir always gets a real folder.
export function worktreeRoot(localPath: string) {
  const r = Bun.spawnSync(["git", "-C", localPath, "config", "worktree.root"]);
  const set = r.exitCode === 0 ? r.stdout.toString().trim() : "";
  const root = resolve(localPath, set ? set.replace(/^~(?=\/|$)/, homedir()) : "../worktrees");
  mkdirSync(root, { recursive: true });
  return root;
}

function workerServer(todoId: number, dir: string, turn: { posted: boolean }) {
  return createSdkMcpServer({
    name: "app",
    version: "1.0.0",
    tools: [
      tool("post_message", "Post a one or two line chat message to the user. Pass file_name to link a file you registered with attach_file.",
        { body: z.string(), file_name: z.string().optional() }, async ({ body, file_name }) => {
          if (!body.trim()) return err("body is empty");
          if (lineCount(body) > 2) return err(`Chat messages are one or two lines. Put the longer content in a markdown file in ${dir}, register it with mcp__app__attach_file, and link it with file_name.`);
          let fileId: number | null = null;
          if (file_name) {
            const f = filesOf(todoId).find((x) => basename(x.path) === basename(file_name));
            if (!f) return err(`No attached file is named ${file_name}. Call mcp__app__attach_file first.`);
            fileId = f.id;
          }
          addMessage(todoId, "agent", body.trim(), "delivered", fileId);
          addEvent(todoId, "post_message", "agent", file_name ?? null);
          turn.posted = true;
          return ok();
        }),
      tool("set_title", "Set this TODO's title: a short summary of the task.", { title: z.string() }, async ({ title }) => {
        if (!title.trim()) return err("title is empty");
        setTodo(todoId, { title: title.trim() });
        addEvent(todoId, "set_title", "agent", title.trim());
        return ok();
      }),
      tool("set_attention", "Raise the attention flag (raised: true) when you need the user; clear it (raised: false) once their reply resolves it.",
        { raised: z.boolean() }, async ({ raised }) => {
          setTodo(todoId, { attention: raised ? 1 : 0 });
          addEvent(todoId, raised ? "attention raised" : "attention cleared", "agent");
          return ok();
        }),
      tool("attach_file", `Register a file you wrote in this TODO's artifact folder (${dir}) so the user can open it. Pass its absolute path.`,
        { path: z.string() }, async ({ path }) => {
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
        }),
      tool("set_worktree", "Register the absolute path of the git worktree this TODO works in.", { path: z.string() }, async ({ path }) => {
        if (!isAbsolute(path) || !existsSync(path) || !statSync(path).isDirectory()) return err(`${path} is not an existing directory (absolute path required)`);
        setTodo(todoId, { worktree_path: resolve(path) });
        addEvent(todoId, "set_worktree", "agent", resolve(path));
        return ok();
      }),
      tool("set_pr", "Register the number of the GitHub pull request this TODO works on, in the project's repository. The app shows its state, checks and reviews.",
        { number: z.number().int().positive() }, async ({ number }) => {
          setTodo(todoId, { pr_number: number });
          addEvent(todoId, "set_pr", "agent", `#${number}`);
          refreshPrs(todoRow(todoId)!.project_id);
          return ok();
        }),
    ],
  });
}

// `takeBodies` marks the messages delivered, so it is called only once the run is set up: a setup error leaves them
// pending or queued, and the user sees an app message instead of messages that look sent but never reached the agent.
function runWorker(todoId: number, takeBodies: () => string[], session: { sessionId: string } | { resume: string }, reason: string, header = "") {
  try {
    setupWorker(todoId, takeBodies, session, reason, header);
  } catch (e: any) {
    addMessage(todoId, "app", cutToTwoLines(`Could not start the run: ${e?.message ?? e}`));
    addEvent(todoId, "error", "app", String(e?.stack ?? e));
  }
}

function setupWorker(todoId: number, takeBodies: () => string[], session: { sessionId: string } | { resume: string }, reason: string, header: string) {
  const t = todoRow(todoId)!;
  const p = projectRow(t.project_id)!;
  const dir = artifactDir(p.slug, todoId);
  mkdirSync(dir, { recursive: true });
  const turn = { posted: false };
  const args = new Map<string, string>(); // tool_use_id -> command, for the denial message
  const denied = new Set<string>();
  const abort = new AbortController();
  const options: Options = {
    ...session,
    cwd: p.local_path,
    permissionMode: "auto",
    // The worktree root lets a `cd` into the worktree persist; the artifact folder is outside cwd and must be writable.
    additionalDirectories: [worktreeRoot(p.local_path), dir],
    mcpServers: { app: workerServer(todoId, dir, turn) },
    allowedTools: WORKER_TOOLS,
    systemPrompt: { type: "preset", preset: "claude_code", append: [CONTRACT.replaceAll("{artifact_dir}", dir), p.agent_prompt.trim()].filter(Boolean).join("\n\n") },
    // Without this, a resumed session keeps the system prompt of its first turn and never sees an edited project prompt.
    extraArgs: { "system-prompt-snapshot": "off" },
    abortController: abort,
    // Hooks only record events and the last tool. Status comes from the run itself, because Stop does not fire on interrupt.
    hooks: {
      UserPromptSubmit: [{ hooks: [async () => { addEvent(todoId, "UserPromptSubmit", "agent"); return {}; }] }],
      PreToolUse: [{
        hooks: [async (input: any) => {
          args.set(input.tool_use_id, toolArg(input.tool_input));
          setTodo(todoId, { last_tool: input.tool_name }, false);
          addEvent(todoId, "PreToolUse", "agent", toolLine(input.tool_name, input.tool_input));
          return {};
        }],
      }],
    },
  };

  // Denials come as a system message and again in the result, so each tool call is reported once.
  const deny = (toolName: string, toolUseId: string, fallback: string) => {
    if (denied.has(toolUseId)) return;
    denied.add(toolUseId);
    addMessage(todoId, "app", `Auto mode blocked ${toolName}: ${args.get(toolUseId) || fallback}`);
    setTodo(todoId, { attention: 1 });
    addEvent(todoId, "attention raised", "app", `${toolName} denied`);
  };

  const q = query({ prompt: deliveryPrompt(t.worktree_path, takeBodies(), header), options });
  const run: Run = { q, abort, done: Promise.resolve() };
  running.set(todoId, run);
  setTodo(todoId, { status: "running", last_tool: null });
  addEvent(todoId, "launch", "app", `${reason} · cwd ${p.local_path} · session ${t.session_id}`);

  // The context meter (item 123) and the result footer (item 128) come from the run's own messages.
  let ctx: Ctx | null = t.ctx ? JSON.parse(t.ctx) : null;
  const res = { duration_ms: null as number | null, tools: 0 };
  run.done = (async () => {
    const { text, error } = await drain(q, (m) => {
      if (m.type === "system" && m.subtype === "permission_denied") deny(m.tool_name, m.tool_use_id, m.message ?? "");
      if (m.type === "result") for (const d of m.permission_denials ?? []) deny(d.tool_name, d.tool_use_id, toolArg(d.tool_input));
      if (m.type === "result") res.duration_ms = m.duration_ms ?? null;
      if (m.type === "assistant" && m.parent_tool_use_id == null) res.tools += (m.message?.content ?? []).filter((b: any) => b.type === "tool_use").length;
      ctx = usageStep(ctx, m);
    });
    // From here to deliver() nothing awaits, so a message committed meanwhile is either queued and picked up below,
    // or sees the TODO idle and starts its own turn.
    running.delete(todoId);
    const stopped = interrupting.delete(todoId);
    if (ctx?.used) setTodo(todoId, { ctx: JSON.stringify(ctx) }, false);
    if (text) addEvent(todoId, "result", "agent", text, res);
    if (stopped) addEvent(todoId, "interrupted", "app");
    else if (error) {
      addMessage(todoId, "app", cutToTwoLines(error));
      addEvent(todoId, "error", "app", error);
    } else if (!turn.posted && text.trim()) addMessage(todoId, "agent", cutToTwoLines(text));
    const status = statusAfterRun(todoRow(todoId)!.attention);
    setTodo(todoId, { status });
    addEvent(todoId, "run ended", "app", status);
    refreshPrs(p.id); // the agent may have pushed or opened a PR
    if (!shuttingDown) deliver(todoId, "queued messages");
  })();
}

// ---------- operator ----------

export function commitOperatorMessage(body: string) {
  const id = addMessage(null, "user", body, "queued");
  deliverOperator();
  return id;
}

function deliverOperator() {
  if (running.has("operator")) return;
  const bodies = take(null, "queued");
  if (bodies.length) runOperator(bodies);
}

const json = (v: unknown) => ok(JSON.stringify(v, null, 1));
const latest = (todoId: number, n: number) =>
  db.query("SELECT author, body, committed_at AS at FROM messages WHERE todo_id = ? ORDER BY id DESC LIMIT ?").all(todoId, n).reverse();

function operatorServer() {
  const used = (name: string, detail: string | null = null) => addEvent(null, name, "operator", detail);
  return createSdkMcpServer({
    name: "app",
    version: "1.0.0",
    tools: [
      tool("list_todos", "List the TODOs: title, project, status (not_started = in the backlog), attention, branch, git state, pull request, session count, files and the latest messages. Removed TODOs only with include_removed.",
        { include_removed: z.boolean().optional() }, async ({ include_removed }) => {
          used("list_todos");
          const list = await summaries(include_removed ? "1" : "t.removed_at IS NULL");
          return json(list.map((s) => ({
            id: s.id, title: s.title, project: s.project_name, status: s.status, attention: s.attention, branch: s.branch, git: s.git, pr: s.pr,
            worktree_path: s.worktree_path, sessions: s.session_count, queued_messages: s.queued_count, pending_messages: s.pending_count, files: s.file_names,
            updated_at: s.updated_at, removed_at: s.removed_at, latest_messages: latest(s.id, 3),
          })));
        }),
      tool("read_todo", "Read one TODO (removed ones too): its details, its sessions with their chat messages, and its file list.",
        { todo_id: z.number().int() }, async ({ todo_id }) => {
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
        }),
      tool("read_file", "Read one of a TODO's attached markdown files by its name.", { todo_id: z.number().int(), name: z.string() },
        async ({ todo_id, name }) => {
          used("read_file", `#${todo_id} ${name}`);
          const f = filesOf(todo_id).find((x) => basename(x.path) === name);
          if (!f) return err(`TODO #${todo_id} has no file named ${name}`);
          if (!existsSync(f.path)) return err(`${f.path} no longer exists on disk`);
          return ok(readFileSync(f.path, "utf8"));
        }),
      tool("create_todo", "Create a TODO in the named project with its first instruction. It stays not started; the user starts it.",
        { project: z.string(), instruction: z.string() }, async ({ project, instruction }) => {
          const all = db.query("SELECT id, name FROM projects").all() as { id: number; name: string }[];
          const p = all.find((x) => x.name === project) ?? all.find((x) => x.name.toLowerCase() === project.trim().toLowerCase());
          if (!p) return err(`Unknown project "${project}". Projects: ${all.map((x) => x.name).join(", ") || "none"}`);
          if (!instruction.trim()) return err("instruction is empty");
          const id = createTodo(p.id, instruction.trim(), "operator");
          used("create_todo", `#${id} in ${p.name}`);
          return ok(`Created TODO #${id} in ${p.name}. It is in the backlog until the user starts it.`);
        }),
      tool("send_message", "Send a message to a TODO on the user's behalf. It is delivered like a message the user commits.",
        { todo_id: z.number().int(), body: z.string() }, async ({ todo_id, body }) => {
          const t = todoRow(todo_id);
          if (!t || t.removed_at) return err(`no live TODO #${todo_id}`);
          if (!body.trim()) return err("body is empty");
          const outcome = { pending: "stored; it is delivered when the user starts the TODO", queue: "queued; it is delivered when the agent's current run ends", send: "delivered now" }[deliveryAction(t.status)];
          commitMessage(todo_id, "user via operator", body.trim());
          used("send_message", `#${todo_id}`);
          return ok(`Message ${outcome}.`);
        }),
    ],
  });
}

function runOperator(bodies: string[]) {
  // The stored id is set on a session's first turn and cleared by POST /api/operator/new-session.
  const stored = getMeta("operator_session_id");
  const sessionId = stored ?? crypto.randomUUID();
  if (!stored) setMeta("operator_session_id", sessionId);
  const abort = new AbortController();
  const q = query({
    prompt: bodies.join("\n\n"),
    options: {
      ...(stored ? { resume: stored } : { sessionId }),
      cwd: HOME,
      tools: OPERATOR_BUILTINS, // no Edit, Write or Bash
      allowedTools: [...OPERATOR_BUILTINS, ...OPERATOR_TOOLS],
      strictMcpConfig: true, // only its own app server, none of the user's MCP servers
      mcpServers: { app: operatorServer() },
      permissionMode: "dontAsk", // anything not listed above is refused instead of asked about
      systemPrompt: { type: "preset", preset: "claude_code", append: OPERATOR_PROMPT(HOME) },
      extraArgs: { "system-prompt-snapshot": "off" },
      abortController: abort,
    },
  });
  const run: Run = { q, abort, done: Promise.resolve() };
  running.set("operator", run);
  emit({ type: "operator" });

  let ctx: Ctx | null = stored && getMeta("operator_ctx") ? JSON.parse(getMeta("operator_ctx")!) : null;
  run.done = (async () => {
    const { text, error } = await drain(q, (m) => { ctx = usageStep(ctx, m); });
    running.delete("operator");
    if (ctx?.used) setMeta("operator_ctx", JSON.stringify(ctx));
    const stopped = interrupting.delete("operator");
    if (!stopped && error) addMessage(null, "app", cutToTwoLines(error));
    else if (!stopped && text.trim()) addMessage(null, "operator", text.trim());
    emit({ type: "operator" });
    if (!shuttingDown) deliverOperator();
  })();
}
