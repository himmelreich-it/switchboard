import { getSessionInfo } from "@anthropic-ai/claude-agent-sdk";
import { Database } from "bun:sqlite";
import { existsSync, mkdirSync, readFileSync, realpathSync, statSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { basename, join, resolve } from "node:path";
import { slugify, statusAfterRun, uniqueSlug, type Status } from "./lib";
import { CONTRACT, commitCoordinatorMessage, commitMessage, deliver, interrupt, isRunning, start, stopAll } from "./runner";

// No login on this server: it must only ever listen on 127.0.0.1.
const HOST = "127.0.0.1";
const PORT = Number(process.env.PORT ?? 4477);

// ---------- storage ----------

// Resolved once (macOS /tmp is /private/tmp), so attach_file can compare real paths against it.
mkdirSync(process.env.AGENT_COORDINATOR_HOME ?? join(homedir(), ".agent-coordinator"), { recursive: true });
export const HOME = realpathSync(process.env.AGENT_COORDINATOR_HOME ?? join(homedir(), ".agent-coordinator"));
const dbPath = join(HOME, "db.sqlite");
const fresh = !existsSync(dbPath);
export const db = new Database(dbPath);
db.exec(`
CREATE TABLE IF NOT EXISTS projects (
  id INTEGER PRIMARY KEY, name TEXT NOT NULL, slug TEXT NOT NULL UNIQUE, local_path TEXT NOT NULL,
  agent_prompt TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS todos (
  id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id), title TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('not_started','running','waiting','idle')), attention INTEGER NOT NULL DEFAULT 0,
  session_id TEXT, worktree_path TEXT, last_tool TEXT, last_read_at TEXT,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL, removed_at TEXT, removed_by TEXT);
CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY, todo_id INTEGER,
  author TEXT NOT NULL CHECK (author IN ('user','agent','user via coordinator','coordinator','app')), body TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('pending','queued','delivered')), file_id INTEGER, committed_at TEXT NOT NULL, delivered_at TEXT);
CREATE TABLE IF NOT EXISTS drafts (key TEXT PRIMARY KEY, body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS files (id INTEGER PRIMARY KEY, todo_id INTEGER NOT NULL, path TEXT NOT NULL UNIQUE);
CREATE TABLE IF NOT EXISTS events (
  id INTEGER PRIMARY KEY, todo_id INTEGER, kind TEXT NOT NULL,
  actor TEXT NOT NULL CHECK (actor IN ('user','agent','app','coordinator')), detail TEXT, at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);
CREATE INDEX IF NOT EXISTS messages_todo ON messages (todo_id, state);
CREATE INDEX IF NOT EXISTS events_todo ON events (todo_id);
`);

export type Todo = {
  id: number; project_id: number; title: string; status: Status; attention: number; session_id: string | null;
  worktree_path: string | null; last_tool: string | null; last_read_at: string | null; created_at: string; updated_at: string;
  removed_at: string | null; removed_by: string | null;
};
export type Project = { id: number; name: string; slug: string; local_path: string; agent_prompt: string; created_at: string };

export const now = () => new Date().toISOString();
export const todoRow = (id: number) => db.query("SELECT * FROM todos WHERE id = ?").get(id) as Todo | null;
export const projectRow = (id: number) => db.query("SELECT * FROM projects WHERE id = ?").get(id) as Project | null;
export const filesOf = (todoId: number) => db.query("SELECT id, path FROM files WHERE todo_id = ? ORDER BY id").all(todoId) as { id: number; path: string }[];
export const artifactDir = (slug: string, todoId: number) => join(HOME, "data", slug, String(todoId));

export const getMeta = (key: string) => (db.query("SELECT value FROM meta WHERE key = ?").get(key) as any)?.value as string | undefined;
export function setMeta(key: string, value: string | null) {
  if (value === null) db.run("DELETE FROM meta WHERE key = ?", [key]);
  else db.run("INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [key, value]);
}
const getDraft = (key: string) => (db.query("SELECT body FROM drafts WHERE key = ?").get(key) as any)?.body ?? "";
export function setDraft(key: string, body: string) {
  if (!body.trim()) db.run("DELETE FROM drafts WHERE key = ?", [key]);
  else db.run("INSERT INTO drafts (key, body) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET body = excluded.body", [key, body]);
}

// ---------- live updates (one SSE stream; the browser refetches what it shows) ----------

const clients = new Set<ReadableStreamDefaultController>();
const enc = new TextEncoder();
const outbox = new Set<string>();
// One change often writes a status, a message and an event; sending each would make the browser refetch three times.
export function emit(ev: object) {
  outbox.add(JSON.stringify(ev));
  if (outbox.size === 1) setTimeout(() => {
    for (const s of outbox) for (const c of clients) try { c.enqueue(enc.encode(`data: ${s}\n\n`)); } catch { clients.delete(c); }
    outbox.clear();
  }, 50);
}
const changed = (todoId: number | null) => emit(todoId === null ? { type: "coordinator" } : { type: "todo", id: todoId });

// `touch` = false for changes that must not move the TODO up its lane (last tool, read marker).
export function setTodo(id: number, fields: Partial<Todo>, touch = true) {
  const f: Record<string, any> = touch ? { ...fields, updated_at: now() } : fields;
  const keys = Object.keys(f);
  db.run(`UPDATE todos SET ${keys.map((k) => `${k} = ?`).join(", ")} WHERE id = ?`, [...keys.map((k) => f[k]), id]);
  changed(id);
}
export function addMessage(todoId: number | null, author: string, body: string, state = "delivered", fileId: number | null = null) {
  const at = now();
  const id = Number(db.run(
    "INSERT INTO messages (todo_id, author, body, state, file_id, committed_at, delivered_at) VALUES (?,?,?,?,?,?,?)",
    [todoId, author, body, state, fileId, at, state === "delivered" ? at : null],
  ).lastInsertRowid);
  if (todoId !== null) db.run("UPDATE todos SET updated_at = ? WHERE id = ?", [at, todoId]);
  changed(todoId);
  return id;
}
export function addEvent(todoId: number | null, kind: string, actor: string, detail: string | null = null) {
  db.run("INSERT INTO events (todo_id, kind, actor, detail, at) VALUES (?,?,?,?,?)", [todoId, kind, actor, detail, now()]);
  changed(todoId);
}

// ---------- shell ----------

async function sh(cmd: string[]) {
  const p = Bun.spawn(cmd, { stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const [out, err, code] = await Promise.all([new Response(p.stdout).text(), new Response(p.stderr).text(), p.exited]);
  return { ok: code === 0, out: out.trim(), err: err.trim() || out.trim() };
}
const branchOf = async (wt: string) => { const r = await sh(["git", "-C", wt, "branch", "--show-current"]); return (r.ok && r.out) || null; };

// ---------- read models ----------

const SUMMARY_SQL = `
SELECT t.*, p.name AS project_name,
  (SELECT COUNT(*) FROM messages m WHERE m.todo_id = t.id) AS message_count,
  (SELECT COUNT(*) FROM messages m WHERE m.todo_id = t.id AND m.state = 'queued') AS queued_count,
  (SELECT COUNT(*) FROM messages m WHERE m.todo_id = t.id AND m.state = 'pending') AS pending_count,
  EXISTS (SELECT 1 FROM messages m WHERE m.todo_id = t.id AND m.author = 'agent'
          AND (t.last_read_at IS NULL OR m.committed_at > t.last_read_at)) AS unread,
  EXISTS (SELECT 1 FROM drafts d WHERE d.key = 'todo:' || t.id) AS has_draft,
  (SELECT json_object('author', author, 'body', body, 'at', committed_at) FROM messages m
   WHERE m.todo_id = t.id ORDER BY m.id DESC LIMIT 1) AS last_message,
  (SELECT json_object('body', body, 'at', committed_at) FROM messages m
   WHERE m.todo_id = t.id AND m.author = 'agent' ORDER BY m.id DESC LIMIT 1) AS last_agent_message,
  (SELECT json_group_array(path) FROM (SELECT path FROM files f WHERE f.todo_id = t.id ORDER BY f.id)) AS file_paths
FROM todos t JOIN projects p ON p.id = t.project_id`;

// ponytail: one `git branch` per TODO with a worktree on every list fetch; cache it if the list gets slow.
export async function summaries(where = "1", args: any[] = []) {
  const rows = db.query(`${SUMMARY_SQL} WHERE ${where} ORDER BY t.updated_at DESC`).all(...args) as any[];
  return Promise.all(rows.map(async ({ file_paths, ...r }) => {
    const names = (JSON.parse(file_paths) as string[]).map((p) => basename(p));
    return {
      ...r, attention: !!r.attention, unread: !!r.unread, has_draft: !!r.has_draft,
      last_message: r.last_message ? JSON.parse(r.last_message) : null,
      last_agent_message: r.last_agent_message ? JSON.parse(r.last_agent_message) : null,
      file_names: names, file_count: names.length,
      branch: r.worktree_path ? await branchOf(r.worktree_path) : null,
    };
  }));
}

function messageOut(id: number) {
  const m = db.query(`SELECT m.id, m.author, m.body, m.state, m.committed_at AS at, m.file_id, f.path AS file_path
    FROM messages m LEFT JOIN files f ON f.id = m.file_id WHERE m.id = ?`).get(id) as any;
  const { file_path, ...rest } = m;
  return { ...rest, file_name: file_path ? basename(file_path) : null };
}

function fileOut(f: { id: number; path: string }) {
  try {
    return { id: f.id, name: basename(f.path), path: f.path, lines: readFileSync(f.path, "utf8").trimEnd().split("\n").length, mtime: statSync(f.path).mtimeMs };
  } catch {
    return { id: f.id, name: basename(f.path), path: f.path, lines: null, mtime: null }; // deleted on disk
  }
}

async function projectOut(p: Project) {
  const r = await sh(["git", "-C", p.local_path, "remote", "get-url", "origin"]);
  const c = db.query(`SELECT EXISTS (SELECT 1 FROM todos WHERE project_id = ? AND session_id IS NOT NULL) AS locked,
    (SELECT COUNT(*) FROM todos WHERE project_id = ? AND removed_at IS NULL) AS todo_count`).get(p.id, p.id) as any;
  return { ...p, repo: r.ok ? r.out : null, locked: !!c.locked, todo_count: c.todo_count };
}

// ---------- writes shared with runner.ts ----------

export function createTodo(projectId: number, instruction: string, actor: "user" | "coordinator") {
  const at = now();
  const id = Number(db.run(
    "INSERT INTO todos (project_id, title, status, created_at, updated_at) VALUES (?, ?, 'not_started', ?, ?)",
    [projectId, instruction, at, at],
  ).lastInsertRowid);
  addMessage(id, actor === "user" ? "user" : "user via coordinator", instruction, "pending");
  addEvent(id, "created", actor, "not started yet");
  emit({ type: "todos" });
  return id;
}

// ---------- migration from auto-coder (only when this database is new) ----------

function migrate() {
  const old = join(homedir(), ".auto-coder", "db.sqlite");
  if (!existsSync(old)) return;
  try {
    const odb = new Database(old, { readonly: true }); // never written: the old reviews and findings are just left behind
    const taken = new Set<string>();
    const projects = odb.query("SELECT name, path FROM projects ORDER BY id").all() as { name: string; path: string }[];
    for (const p of projects) {
      const slug = uniqueSlug(slugify(p.name), taken);
      taken.add(slug);
      db.run("INSERT INTO projects (name, slug, local_path, created_at) VALUES (?, ?, ?, ?)", [p.name, slug, p.path, now()]);
    }
    const prompt = (odb.query("SELECT value FROM settings WHERE key = 'prompt'").get() as any)?.value;
    if (prompt) writeFileSync(join(HOME, "old-review-prompt.md"), prompt);
    odb.close();
    console.log(`imported ${projects.length} projects${prompt ? " and the old review prompt" : ""} from ${old}`);
  } catch (e: any) {
    console.error(`could not import from ${old}: ${e.message}`);
  }
}
if (fresh) migrate();

// ---------- HTTP ----------

const json = (v: unknown, status = 200) => Response.json(v, { status });
const fail = (msg: string, status = 400) => json({ error: msg }, status);
const httpError = (status: number, msg: string) => Object.assign(new Error(msg), { status });

function todoOr404(id: number) {
  const t = todoRow(id);
  if (!t) throw httpError(404, "no such TODO");
  return t;
}
function liveTodo(id: number) {
  const t = todoOr404(id);
  if (t.removed_at) throw httpError(409, "this TODO is removed");
  return t;
}
const expandHome = (p: string) => resolve(p.trim().replace(/^~(?=\/|$)/, homedir()));
async function requireRepo(path: string) {
  const r = await sh(["git", "-C", path, "rev-parse", "--git-dir"]);
  if (!r.ok) throw httpError(400, `${path} is not a git repository`);
}

const RESTORE_DETAIL = "Nothing was started and the TODO stays removed. Claude Code deletes session transcripts after 30 days by default (cleanupPeriodDays in ~/.claude/settings.json).";

async function api(req: Request, path: string): Promise<Response> {
  const method = req.method;
  const body: any = method === "GET" ? null : await req.json().catch(() => ({}));
  let m: RegExpMatchArray | null;

  if (path === "/api/events") {
    let ctl: ReadableStreamDefaultController;
    const ping = setInterval(() => { try { ctl.enqueue(enc.encode(": ping\n\n")); } catch {} }, 20000);
    const stream = new ReadableStream({
      start(c) { ctl = c; clients.add(c); },
      cancel() { clients.delete(ctl); clearInterval(ping); },
    });
    return new Response(stream, { headers: { "content-type": "text/event-stream", "cache-control": "no-cache" } });
  }

  if (path === "/api/meta" && method === "GET") {
    const w = db.query("SELECT COUNT(*) AS n FROM todos WHERE status = 'waiting' AND removed_at IS NULL").get() as any;
    const last = getMeta("last_project_id");
    return json({ last_project_id: last ? Number(last) : null, contract: CONTRACT, waiting_count: w.n });
  }

  // ----- projects -----

  if (path === "/api/projects" && method === "GET")
    return json(await Promise.all((db.query("SELECT * FROM projects ORDER BY name").all() as Project[]).map(projectOut)));
  if (path === "/api/projects" && method === "POST") {
    const name = String(body.name ?? "").trim();
    if (!name || !String(body.local_path ?? "").trim()) return fail("name and local folder are required");
    const localPath = expandHome(body.local_path);
    await requireRepo(localPath);
    const taken = new Set((db.query("SELECT slug FROM projects").all() as any[]).map((r) => r.slug));
    const id = Number(db.run(
      "INSERT INTO projects (name, slug, local_path, agent_prompt, created_at) VALUES (?, ?, ?, ?, ?)",
      [name, uniqueSlug(slugify(name), taken), localPath, String(body.agent_prompt ?? ""), now()],
    ).lastInsertRowid);
    emit({ type: "projects" });
    return json(await projectOut(projectRow(id)!));
  }
  if ((m = path.match(/^\/api\/projects\/(\d+)$/)) && method === "PUT") {
    const p = projectRow(Number(m[1]));
    if (!p) return fail("no such project", 404);
    const name = body.name === undefined ? p.name : String(body.name).trim();
    const localPath = body.local_path === undefined ? p.local_path : expandHome(body.local_path);
    if (!name) return fail("name is required");
    if (localPath !== p.local_path) {
      // Claude Code keeps transcripts under the launch folder, so moving it would orphan every started session.
      if ((await projectOut(p)).locked) return fail("the local folder is read-only once a TODO in this project has started");
      await requireRepo(localPath);
    }
    db.run("UPDATE projects SET name = ?, local_path = ?, agent_prompt = ? WHERE id = ?",
      [name, localPath, body.agent_prompt === undefined ? p.agent_prompt : String(body.agent_prompt), p.id]);
    emit({ type: "projects" });
    emit({ type: "todos" }); // rows show the project name
    return json(await projectOut(projectRow(p.id)!));
  }

  // ----- TODOs -----

  if (path === "/api/todos" && method === "GET") return json(await summaries());
  if (path === "/api/todos" && method === "POST") {
    const instruction = String(body.instruction ?? "").trim();
    if (!projectRow(Number(body.project_id))) return fail("choose a project");
    if (!instruction) return fail("the first instruction is required");
    const id = createTodo(Number(body.project_id), instruction, "user");
    setMeta("last_project_id", String(Number(body.project_id)));
    return json((await summaries("t.id = ?", [id]))[0]);
  }

  if ((m = path.match(/^\/api\/todos\/(\d+)\/files\/(\d+)$/))) {
    const id = Number(m[1]);
    const t = todoOr404(id);
    const f = db.query("SELECT id, path FROM files WHERE id = ? AND todo_id = ?").get(Number(m[2]), id) as { id: number; path: string } | null;
    if (!f) return fail("no such file", 404);
    if (method === "GET") {
      if (!existsSync(f.path)) return fail(`${f.path} no longer exists on disk`, 404);
      return json({ id: f.id, name: basename(f.path), path: f.path, content: readFileSync(f.path, "utf8"), mtime: statSync(f.path).mtimeMs });
    }
    if (method === "PUT") {
      if (typeof body.content !== "string") return fail("content is required");
      if (t.status === "running") return json({ error: "running" }, 409);
      const mtime = existsSync(f.path) ? statSync(f.path).mtimeMs : null;
      if (!body.force && mtime !== body.base_mtime) return json({ error: "changed", mtime }, 409);
      writeFileSync(f.path, body.content); // the agent is not told; the user hands work back with a chat message
      changed(id);
      return json({ mtime: statSync(f.path).mtimeMs });
    }
  }

  if ((m = path.match(/^\/api\/todos\/(\d+)$/)) && method === "GET") {
    const id = Number(m[1]);
    const [todo] = await summaries("t.id = ?", [id]);
    if (!todo) return fail("no such TODO", 404);
    const ids = db.query("SELECT id FROM messages WHERE todo_id = ? ORDER BY id").all(id) as { id: number }[];
    return json({
      todo,
      messages: ids.map((r) => messageOut(r.id)),
      files: filesOf(id).map(fileOut),
      events: db.query("SELECT kind, actor, detail, at FROM events WHERE todo_id = ? ORDER BY id").all(id),
      draft: getDraft(`todo:${id}`),
    });
  }

  if ((m = path.match(/^\/api\/todos\/(\d+)\/draft$/)) && method === "PUT") {
    const id = todoOr404(Number(m[1])).id;
    setDraft(`todo:${id}`, String(body.body ?? ""));
    changed(id);
    return json({});
  }

  if ((m = path.match(/^\/api\/todos\/(\d+)\/(start|messages|read|clear-attention|remove|restore|deliver)$/)) && method === "POST") {
    const id = Number(m[1]);
    const action = m[2];

    if (action === "start") {
      if (liveTodo(id).status !== "not_started") return fail("this TODO has already started", 409);
      start(id);
    } else if (action === "messages") {
      liveTodo(id);
      const text = String(body.body ?? "").trim();
      if (!text) return fail("empty message");
      const mid = commitMessage(id, "user", text);
      setDraft(`todo:${id}`, "");
      return json(messageOut(mid));
    } else if (action === "read") {
      // Only an actual change is announced, so a page that marks read on every refetch cannot loop.
      const [s] = await summaries("t.id = ?", [todoOr404(id).id]);
      if (s.unread) setTodo(id, { last_read_at: now() }, false);
      else db.run("UPDATE todos SET last_read_at = ? WHERE id = ?", [now(), id]);
    } else if (action === "clear-attention") {
      const t = todoOr404(id);
      if (t.attention) {
        setTodo(id, { attention: 0, status: t.status === "waiting" ? "idle" : t.status });
        addEvent(id, "attention cleared", "user");
      }
    } else if (action === "remove") {
      liveTodo(id);
      setTodo(id, { removed_at: now(), removed_by: "user" });
      addEvent(id, "removed", "user");
      interrupt(id); // like Esc in Claude Code; queued messages stay queued
      emit({ type: "todos" });
    } else if (action === "restore") {
      const t = todoOr404(id);
      if (!t.removed_at) return fail("this TODO is not removed", 409);
      if (t.session_id) {
        if (!(await getSessionInfo(t.session_id)))
          return json({ warning: `The Claude Code session ${t.session_id} no longer exists.`, detail: RESTORE_DETAIL }, 409);
        if (t.worktree_path && !existsSync(t.worktree_path))
          return json({ warning: `The worktree ${t.worktree_path} no longer exists.`, detail: RESTORE_DETAIL }, 409);
        const folder = projectRow(t.project_id)!.local_path;
        if (!existsSync(folder)) return json({ warning: `The project folder ${folder} no longer exists.`, detail: RESTORE_DETAIL }, 409);
      }
      setTodo(id, { removed_at: null, removed_by: null });
      addEvent(id, "restored", "user");
      emit({ type: "todos" });
      // A never-started TODO goes back to not started with its pending messages; nothing launches.
      if (t.session_id) {
        // Queued messages plus the draft go out as one prompt; with neither, the prompt is "continue". "continue" is
        // queued like any message, so a restore while the interrupted run is still ending goes out when that run ends.
        const draft = getDraft(`todo:${id}`).trim();
        const queued = (db.query("SELECT COUNT(*) AS n FROM messages WHERE todo_id = ? AND state = 'queued'").get(id) as { n: number }).n;
        if (draft || !queued) addMessage(id, "user", draft || "continue", "queued");
        setDraft(`todo:${id}`, "");
        deliver(id, "restore");
      }
    } else if (action === "deliver") {
      if (isRunning(liveTodo(id).id)) return fail("the agent is running; queued messages go out when it stops", 409);
      deliver(id, "deliver");
    }
    return json({});
  }

  // ----- coordinator -----

  if (path === "/api/coordinator" && method === "GET") {
    const since = getMeta("coordinator_since") ?? "";
    return json({
      status: isRunning("coordinator") ? "running" : "idle",
      session_id: getMeta("coordinator_session_id") ?? null,
      messages: db.query("SELECT id, author, body, committed_at AS at, state FROM messages WHERE todo_id IS NULL AND committed_at >= ? ORDER BY id").all(since),
      draft: getDraft("coordinator"),
    });
  }
  if (path === "/api/coordinator/messages" && method === "POST") {
    const text = String(body.body ?? "").trim();
    if (!text) return fail("empty message");
    const id = commitCoordinatorMessage(text);
    setDraft("coordinator", "");
    return json(db.query("SELECT id, author, body, committed_at AS at, state FROM messages WHERE id = ?").get(id));
  }
  if (path === "/api/coordinator/draft" && method === "PUT") {
    setDraft("coordinator", String(body.body ?? ""));
    changed(null);
    return json({});
  }
  if (path === "/api/coordinator/new-session" && method === "POST") {
    if (isRunning("coordinator")) return fail("the coordinator is running; wait for its reply", 409);
    // The next message starts a new session id. Older messages stay in the database but are hidden.
    setMeta("coordinator_session_id", null);
    setMeta("coordinator_since", now());
    addEvent(null, "new coordinator session", "user");
    return json({});
  }

  return fail("not found", 404);
}

Bun.serve({
  hostname: HOST,
  port: PORT,
  idleTimeout: 0, // the default 10s would cut the SSE stream
  async fetch(req) {
    const url = new URL(req.url);
    // Blocks DNS-rebinding pages: the Host header must name this loopback server.
    if (![`${HOST}:${PORT}`, `localhost:${PORT}`].includes(req.headers.get("host") ?? "")) return fail("bad host", 403);
    // Cross-site forms can only send simple content types; requiring JSON forces a CORS preflight, which this server never answers.
    if (req.method !== "GET" && !req.headers.get("content-type")?.startsWith("application/json")) return fail("JSON body required", 415);
    if (url.pathname === "/") return new Response(Bun.file(join(import.meta.dir, "index.html")));
    if (url.pathname === "/favicon.svg") return new Response(Bun.file(join(import.meta.dir, "favicon.svg")));
    try {
      return await api(req, url.pathname);
    } catch (e: any) {
      return fail(e.message ?? String(e), e.status ?? 500);
    }
  },
});
console.log(`agent-coordinator on http://${HOST}:${PORT} (data in ${HOME})`);

// A run cannot survive a restart. Queued messages stay queued until the user presses Deliver.
// Only after Bun.serve has bound the port: a second copy that fails with EADDRINUSE must not touch the live app's TODOs.
for (const t of db.query("SELECT id, attention FROM todos WHERE status = 'running'").all() as Todo[]) {
  setTodo(t.id, { status: statusAfterRun(t.attention) });
  addEvent(t.id, "app restarted", "app");
}

// Interrupts every running agent so none keeps working unseen; a second Ctrl-C exits at once.
let stopping = false;
async function shutdown() {
  if (stopping) process.exit(1);
  stopping = true;
  console.log("stopping: interrupting running agents");
  await Promise.race([stopAll(), Bun.sleep(5000)]);
  process.exit(0);
}
process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);
