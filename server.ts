import type { ServerWebSocket, Subprocess } from "bun";
import { Database } from "bun:sqlite";
import { chmodSync, existsSync, lstatSync, mkdirSync, readdirSync, readFileSync, realpathSync, renameSync, statSync, unlinkSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve } from "node:path";
import {
  MODES, configIn, contextFile, defaultsOf, ghOutcome, isInside, migrateDb, parseGitStatus, parseMarks, parseOscTitle, revealCommand, versionAtLeast, parseGithubRemote, parseRefCounts, prFromNode, prQuery, safeFileName, slugify, statusAfterRun, tokenOk, uniqueSlug,
  type ConfigOption, type Pr, type Status,
} from "./lib";
import { authenticate, describe, sessionExists } from "./acp";
import { DEFAULT_AGENT, agentList, onPath, agentName, isBuiltin, isClaude, isKnown, resumeHint, specOf } from "./agents";
import { WORKER_TOOLS, grant, handleMcp, revoke } from "./apptools";
import { CONTRACT, DEFAULT_BACKGROUND_HOURS, TERMINAL_CONTRACT, agentConfigs, answerPermission, applyConfig, backgroundCount, closeAgent, metaKey, commandsOf, commitMessage, commitOperatorMessage, deliver, interrupt, isRunning, liveAgent, modeOf, newSession, start, stopAll, worktreeRoot } from "./runner";

// No login on this server: it must only ever listen on 127.0.0.1.
export const HOST = "127.0.0.1";
export const PORT = Number(process.env.PORT ?? 4477);

// ---------- storage ----------

// Resolved once (macOS /tmp is /private/tmp), so attach_file can compare real paths against it.
mkdirSync(process.env.SWITCHBOARD_HOME ?? join(homedir(), ".switchboard"), { recursive: true });
export const HOME = realpathSync(process.env.SWITCHBOARD_HOME ?? join(homedir(), ".switchboard"));
const dbPath = join(HOME, "db.sqlite");
const fresh = !existsSync(dbPath);
export const db = new Database(dbPath);
migrateDb(db);

export type Todo = {
  id: number; project_id: number; title: string; status: Status; attention: number; session_id: string | null;
  worktree_path: string | null; pr_number: number | null; last_tool: string | null; last_read_at: string | null; created_at: string; updated_at: string;
  removed_at: string | null; removed_by: string | null; ctx: string | null; plan: string | null;
  mode: string | null; permission: string | null; attention_by: string | null; config: string | null; overrides: string | null;
  agent: string | null; suggestions: string | null;
};
export type Session = { no: number; session_id: string; started_at: string; ended_at: string | null; agent: string | null };
export type Project = {
  id: number; name: string; slug: string; local_path: string; agent_prompt: string; created_at: string; mode: string | null; defaults: string | null;
  default_agent: string | null;
};

export const now = () => new Date().toISOString();
export const todoRow = (id: number) => db.query("SELECT * FROM todos WHERE id = ?").get(id) as Todo | null;
export const projectRow = (id: number) => db.query("SELECT * FROM projects WHERE id = ?").get(id) as Project | null;
export const filesOf = (todoId: number) =>
  db.query("SELECT id, path, session_no FROM files WHERE todo_id = ? ORDER BY id").all(todoId) as { id: number; path: string; session_no: number | null }[];
export const artifactDir = (slug: string, todoId: number) => join(HOME, "data", slug, String(todoId));
export const attachmentsOf = (messageId: number) =>
  db.query("SELECT id, name, path FROM attachments WHERE message_id = ? ORDER BY id").all(messageId) as { id: number; name: string; path: string }[];

// Files the user sends with a message, base64 inside the JSON body (see the content-type check in Bun.serve).
export type Upload = { name: string; data: string };
// Written to <artifact folder>/uploads/<message id>-<n>-<name>, so two pasted "image.png" never collide.
export function saveAttachments(todoId: number, messageId: number, uploads: Upload[]) {
  if (!uploads.length) return;
  const dir = join(artifactDir(projectRow(todoRow(todoId)!.project_id)!.slug, todoId), "uploads");
  mkdirSync(dir, { recursive: true });
  uploads.forEach((u, i) => {
    const name = safeFileName(u.name);
    const path = join(dir, `${messageId}-${i + 1}-${name}`);
    writeFileSync(path, Buffer.from(u.data, "base64"));
    db.run("INSERT INTO attachments (message_id, name, path) VALUES (?, ?, ?)", [messageId, name, path]);
  });
}

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
const changed = (todoId: number | null) => emit(todoId === null ? { type: "operator" } : { type: "todo", id: todoId });

// `touch` = false for changes that must not move the TODO up its lane (last tool, read marker).
export function setTodo(id: number, fields: Partial<Todo>, touch = true) {
  const f: Record<string, any> = touch ? { ...fields, updated_at: now() } : fields;
  const keys = Object.keys(f);
  db.run(`UPDATE todos SET ${keys.map((k) => `${k} = ?`).join(", ")} WHERE id = ?`, [...keys.map((k) => f[k]), id]);
  changed(id);
}
// The live session's number (items 68, 77): agent and app messages, events and files belong to the session they
// happen in; a user message gets its session when it is delivered (take() in runner.ts).
export const liveSessionNo = (todoId: number | null) =>
  todoId === null ? null : ((db.query("SELECT MAX(no) AS n FROM sessions WHERE todo_id = ?").get(todoId) as any)?.n ?? null) as number | null;
export const sessionsOf = (todoId: number) =>
  db.query("SELECT no, session_id, started_at, ended_at, agent FROM sessions WHERE todo_id = ? ORDER BY no").all(todoId) as Session[];
export function addMessage(todoId: number | null, author: string, body: string, state = "delivered", fileId: number | null = null, via: string | null = null) {
  const at = now();
  const id = Number(db.run(
    "INSERT INTO messages (todo_id, author, body, state, file_id, committed_at, delivered_at, session_no, via) VALUES (?,?,?,?,?,?,?,?,?)",
    [todoId, author, body, state, fileId, at, state === "delivered" ? at : null, state === "delivered" ? liveSessionNo(todoId) : null, via],
  ).lastInsertRowid);
  if (todoId !== null) db.run("UPDATE todos SET updated_at = ? WHERE id = ?", [at, todoId]);
  changed(todoId);
  return id;
}
export function addEvent(todoId: number | null, kind: string, actor: string, detail: string | null = null, meta: object | null = null) {
  const id = Number(db.run("INSERT INTO events (todo_id, kind, actor, detail, at, session_no, meta) VALUES (?,?,?,?,?,?,?)",
    [todoId, kind, actor, detail, now(), liveSessionNo(todoId), meta && JSON.stringify(meta)]).lastInsertRowid);
  changed(todoId);
  return id;
}
// A transcript row that grows while the agent works (a tool call's status, input, diff and output; the plan).
export function setEvent(id: number, todoId: number, detail: string | null, meta: object | null) {
  db.run("UPDATE events SET detail = ?, meta = ? WHERE id = ?", [detail, meta && JSON.stringify(meta), id]);
  changed(todoId);
}

// ---------- shell ----------

async function sh(cmd: string[], opts: { env?: Record<string, string>; timeout?: number } = {}) {
  const p = Bun.spawn(cmd, { stdin: "ignore", stdout: "pipe", stderr: "pipe", env: opts.env && { ...process.env, ...opts.env }, timeout: opts.timeout });
  const [out, err, code] = await Promise.all([new Response(p.stdout).text(), new Response(p.stderr).text(), p.exited]);
  return { ok: code === 0, code, out: out.trim(), err: err.trim() || out.trim() };
}

// Shows a file in Finder / Explorer. Not awaited: Explorer exits with 1 even when it opened the window.
// Verbatim arguments only matter on Windows; see revealCommand.
function reveal(file: string) {
  if (!existsSync(file)) throw httpError(404, `${file} no longer exists on disk`);
  try { Bun.spawn(revealCommand(process.platform, file), { stdin: "ignore", stdout: "ignore", stderr: "ignore", windowsVerbatimArguments: true }); }
  catch (e: any) { throw httpError(500, `could not open the folder: ${e.message}`); }
  return json({ ok: true });
}

// ---------- git state of a worktree (local, read on every list fetch) ----------

export type GitState = ReturnType<typeof parseGitStatus> & ReturnType<typeof parseRefCounts>;
// Two git processes, ~0.03 s each. --no-optional-locks: a plain `git status` may rewrite the index and hold
// .git/index.lock, which would fail a `git commit` the agent runs in the same worktree at that moment.
// null when the worktree is gone or not a repository.
export async function gitState(wt: string): Promise<GitState | null> {
  const st = await sh(["git", "-C", wt, "--no-optional-locks", "status", "--porcelain=v2", "--branch"]);
  if (!st.ok) return null;
  const s = parseGitStatus(st.out);
  const refs = ["refs/remotes/origin/HEAD", ...(s.branch ? [`refs/remotes/origin/${s.branch}`] : [])];
  const r = await sh(["git", "-C", wt, "for-each-ref", "--format=%(refname) %(symref:short) %(ahead-behind:HEAD)", ...refs]);
  return { ...s, ...parseRefCounts(r.ok ? r.out : "", s.branch) };
}

// ---------- pull requests (GitHub, refreshed every 2 minutes and after each run, never on a list fetch) ----------

type GhError = { kind: "no network" | "gh access failed" | "unclear"; why: string };
type PrStore = { at: string | null; byBranch: Map<string, Pr>; byNumber: Map<number, Pr>; error: GhError | null };
const prStore = new Map<number, PrStore>(); // project id -> the last result; an error keeps the last good PRs
const prInFlight = new Map<number, Promise<void>>();
const ghLoginOf = new Map<number, string>(); // project id -> the gh account that can read its repository
let ghLogins: string[] | null = null;        // every gh account on this machine; read once, again after an access failure

// One call per project for all its worktree TODOs. Each gh account sees only its own repositories, so the call runs
// as each logged-in account in turn (GH_TOKEN for that call only; the active `gh` account is never switched).
export function refreshPrs(projectId: number) {
  let p = prInFlight.get(projectId);
  if (!p) {
    p = fetchPrs(projectId).catch((e) => console.error(`PR refresh for project ${projectId}: ${e?.message ?? e}`)).finally(() => prInFlight.delete(projectId));
    prInFlight.set(projectId, p);
  }
  return p;
}
// A TODO is looked up when it has a worktree (by its branch) or a PR registered with set_pr (by number, also without a worktree).
const PR_TODOS = "removed_at IS NULL AND (worktree_path IS NOT NULL OR pr_number IS NOT NULL)";
export async function refreshAllPrs() {
  const ids = db.query(`SELECT DISTINCT project_id AS id FROM todos WHERE ${PR_TODOS}`).all() as { id: number }[];
  await Promise.all(ids.map((r) => refreshPrs(r.id)));
}

async function fetchPrs(projectId: number) {
  const p = projectRow(projectId);
  const todos = db.query(`SELECT worktree_path, pr_number FROM todos WHERE project_id = ? AND ${PR_TODOS}`)
    .all(projectId) as { worktree_path: string | null; pr_number: number | null }[];
  if (!p || !todos.length) return;
  const remote = await sh(["git", "-C", p.local_path, "remote", "get-url", "origin"]);
  const repo = remote.ok ? parseGithubRemote(remote.out) : null;
  if (!repo) return; // not on GitHub: no PR line
  const numbers = [...new Set(todos.map((t) => t.pr_number).filter((n): n is number => !!n))];
  const branches = [...new Set((await Promise.all(todos.flatMap((t) => (!t.pr_number && t.worktree_path ? [gitState(t.worktree_path)] : []))))
    .map((g) => g?.branch).filter((b): b is string => !!b))];
  if (!numbers.length && !branches.length) return;
  const args = ["gh", "api", "graphql", "-f", `query=${prQuery(branches.length, numbers.length)}`, "-f", `o=${repo.owner}`, "-f", `n=${repo.name}`,
    ...branches.flatMap((b, i) => ["-f", `b${i}=${b}`]), ...numbers.flatMap((n, i) => ["-F", `n${i}=${n}`])];
  const slug = `${repo.owner}/${repo.name}`;
  const prev = prStore.get(projectId) ?? { at: null, byBranch: new Map(), byNumber: new Map(), error: null };
  const fail = (error: GhError) => { prStore.set(projectId, { ...prev, error }); emit({ type: "todos" }); };

  let logins: string[];
  try {
    ghLogins ??= (JSON.parse((await sh(["gh", "auth", "status", "--json", "hosts"], { timeout: 15_000 })).out || "{}").hosts?.["github.com"] ?? [])
      .map((h: any) => h.login);
    logins = ghLogins!;
  } catch {
    return fail({ kind: "gh access failed", why: "gh is not installed or did not answer. Install it, run gh auth login, then retry." });
  }
  const known = ghLoginOf.get(projectId);
  for (const login of known ? [known, ...logins.filter((l) => l !== known)] : logins) {
    const token = await sh(["gh", "auth", "token", "--user", login], { timeout: 15_000 });
    if (!token.ok) continue;
    const r = await sh(args, { env: { GH_TOKEN: token.out }, timeout: 15_000 });
    const o = ghOutcome(r.code, r.out, r.err);
    if ("error" in o && o.error === "no access") continue;
    if ("error" in o)
      return fail(o.error === "no network" ? { kind: "no network", why: "Can't reach github.com." }
        : { kind: "unclear", why: `gh answered, but not in a form Switchboard can read (${o.detail}). Usually passes on the next try.` });
    ghLoginOf.set(projectId, login);
    const newest = (i: number) => o.repo[`b${i}`]?.nodes?.[0]; // pullRequests(...) returns a list
    prStore.set(projectId, {
      at: now(), error: null,
      byBranch: new Map(branches.flatMap((b, i) => (newest(i) ? [[b, prFromNode(newest(i))]] : []))),
      byNumber: new Map(numbers.flatMap((n, i) => (o.repo[`n${i}`] ? [[n, prFromNode(o.repo[`n${i}`])]] : []))),
    });
    emit({ type: "todos" });
    return;
  }
  ghLogins = null; // a `gh auth login` since then is seen on the next try
  ghLoginOf.delete(projectId);
  fail({ kind: "gh access failed", why: `gh could not open ${slug} with any logged-in account (${logins.join(", ") || "none"}). Run gh auth login, then retry.` });
}

// ---------- read models ----------

const SUMMARY_SQL = `
SELECT t.*, p.name AS project_name,
  (SELECT COUNT(*) FROM messages m WHERE m.todo_id = t.id) AS message_count,
  (SELECT COUNT(*) FROM messages m WHERE m.todo_id = t.id AND m.state = 'queued') AS queued_count,
  (SELECT COUNT(*) FROM messages m WHERE m.todo_id = t.id AND m.state = 'pending') AS pending_count,
  (SELECT COUNT(*) FROM attachments a JOIN messages m ON m.id = a.message_id WHERE m.todo_id = t.id AND m.state = 'pending') AS pending_attachments,
  (SELECT COUNT(*) FROM sessions s WHERE s.todo_id = t.id) AS session_count,
  (SELECT author FROM messages m WHERE m.todo_id = t.id ORDER BY m.id LIMIT 1) AS creator,
  EXISTS (SELECT 1 FROM messages m WHERE m.todo_id = t.id AND m.author = 'agent'
          AND (t.last_read_at IS NULL OR m.committed_at > t.last_read_at)) AS unread,
  EXISTS (SELECT 1 FROM drafts d WHERE d.key = 'todo:' || t.id) AS has_draft,
  (SELECT json_object('author', author, 'body', body, 'at', committed_at,
     'files', (SELECT group_concat(name, ', ') FROM (SELECT name FROM attachments a WHERE a.message_id = m.id ORDER BY a.id))) FROM messages m
   WHERE m.todo_id = t.id ORDER BY m.id DESC LIMIT 1) AS last_message,
  (SELECT json_object('body', body, 'at', committed_at) FROM messages m
   WHERE m.todo_id = t.id AND m.author = 'agent' ORDER BY m.id DESC LIMIT 1) AS last_agent_message,
  (SELECT json_group_array(path) FROM (SELECT path FROM files f WHERE f.todo_id = t.id ORDER BY f.id)) AS file_paths
FROM todos t JOIN projects p ON p.id = t.project_id`;

// ponytail: two git processes per TODO with a worktree on every list fetch; cache gitState() per path for a few seconds
// if the list gets slow. PRs come from prStore in memory, never from a fetch.
export async function summaries(where = "1", args: any[] = []) {
  const rows = db.query(`${SUMMARY_SQL} WHERE ${where} ORDER BY t.updated_at DESC`).all(...args) as any[];
  return Promise.all(rows.map(async ({ file_paths, ctx, permission, config, overrides, suggestions, ...r }) => {
    const names = (JSON.parse(file_paths) as string[]).map((p) => basename(p));
    const git = r.worktree_path ? await gitState(r.worktree_path) : null;
    const store = prStore.get(r.project_id);
    // set_pr wins over the branch: a review worktree's local branch need not match the PR's head branch.
    const pr = store ? (r.pr_number ? store.byNumber.get(r.pr_number) : git?.branch ? store.byBranch.get(git.branch) : null) : null;
    return {
      ...r, agent: r.agent || DEFAULT_AGENT, attention: !!r.attention, unread: !!r.unread, has_draft: !!r.has_draft,
      last_message: r.last_message ? JSON.parse(r.last_message) : null,
      last_agent_message: r.last_agent_message ? JSON.parse(r.last_agent_message) : null,
      file_names: names, file_count: names.length, ctx: ctx ? JSON.parse(ctx) : null, plan: r.plan ? JSON.parse(r.plan) : null,
      permission: permission ? JSON.parse(permission) : null,
      config: config ? JSON.parse(config) : null, overrides: overrides ? JSON.parse(overrides) : {},
      suggestions: suggestions ? JSON.parse(suggestions) : null,
      branch: git?.branch ?? null, git, pr: pr ? { ...pr, via: r.pr_number ? "set_pr" : null } : null,
      pr_checked_at: store?.at ?? null, gh_error: store?.error ?? null,
      // Item 136: the agent's background tasks still running, and whether the TODO shows running only for them.
      background: backgroundCount(r.id), background_wait: r.status === "running" && !isRunning(r.id) && backgroundCount(r.id) > 0,
    };
  }));
}

function messageOut(id: number) {
  const m = db.query(`SELECT m.id, m.author, m.body, m.state, m.committed_at AS at, m.file_id, m.session_no, m.via, f.path AS file_path
    FROM messages m LEFT JOIN files f ON f.id = m.file_id WHERE m.id = ?`).get(id) as any;
  const { file_path, ...rest } = m;
  return { ...rest, file_name: file_path ? basename(file_path) : null, attachments: attachmentsOf(id).map(({ id, name, path }) => ({ id, name, size: Bun.file(path).size })) };
}

function fileOut(f: { id: number; path: string; session_no: number | null }) {
  try {
    return { id: f.id, name: basename(f.path), path: f.path, session_no: f.session_no, lines: readFileSync(f.path, "utf8").trimEnd().split("\n").length, mtime: statSync(f.path).mtimeMs };
  } catch {
    return { id: f.id, name: basename(f.path), path: f.path, session_no: f.session_no, lines: null, mtime: null }; // deleted on disk
  }
}

// Item 85: every file in the artifact folder is a TODO file, also one written from a terminal. Files no session
// registered get session_no NULL ("added outside a session"). uploads/ (attachments) and terminal/ (context) are skipped.
// Symlinks are skipped: they could point the editor at files outside the folder, or loop.
// ponytail: stops after 500 files; an artifact folder holds plans and notes, not a checkout.
function registerDiskFiles(t: Todo) {
  const dir = artifactDir(projectRow(t.project_id)!.slug, t.id);
  if (!existsSync(dir)) return;
  const found: string[] = [];
  const walk = (d: string, top: boolean) => {
    let names: string[];
    try { names = readdirSync(d); } catch { return; }
    for (const n of names) {
      if (found.length >= 500 || n.startsWith(".") || (top && (n === "uploads" || n === "terminal"))) continue;
      const p = join(d, n);
      try {
        const st = lstatSync(p);
        if (st.isDirectory()) walk(p, false);
        else if (st.isFile()) found.push(p);
      } catch {}
    }
  };
  walk(realpathSync(dir), true);
  for (const p of found) db.run("INSERT OR IGNORE INTO files (todo_id, path) VALUES (?, ?)", [t.id, p]);
}

async function projectOut(p: Project) {
  const r = await sh(["git", "-C", p.local_path, "remote", "get-url", "origin"]);
  const c = db.query(`SELECT EXISTS (SELECT 1 FROM todos WHERE project_id = ? AND session_id IS NOT NULL) AS locked,
    (SELECT COUNT(*) FROM todos WHERE project_id = ? AND removed_at IS NULL) AS todo_count`).get(p.id, p.id) as any;
  return { ...p, default_agent: p.default_agent ?? DEFAULT_AGENT, defaults: defaultsOf(p.defaults), repo: r.ok ? r.out : null, locked: !!c.locked, todo_count: c.todo_count };
}

// ---------- writes shared with runner.ts ----------

// One transaction, so a failed file write leaves no TODO behind.
// `agent` is the agent of the first session (P5): the project's default when not given.
export function createTodo(projectId: number, instruction: string, actor: "user" | "operator", uploads: Upload[] = [], agent: string | null = null) {
  const id = db.transaction(() => {
    const at = now();
    const id = Number(db.run(
      "INSERT INTO todos (project_id, title, status, created_at, updated_at, agent) VALUES (?, ?, 'not_started', ?, ?, ?)",
      [projectId, instruction, at, at, agent ?? projectRow(projectId)!.default_agent ?? DEFAULT_AGENT],
    ).lastInsertRowid);
    saveAttachments(id, addMessage(id, actor === "user" ? "user" : "user via operator", instruction, "pending"), uploads);
    return id;
  })();
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
// A permission mode from a request body: null for "not set" (empty or null), the mode when known, undefined when not.
// Claude's modes are known here; another agent's are its own, so any name is taken and the agent reports a miss.
const modeIn = (v: unknown, agent: string | null): string | null | undefined =>
  v == null || v === "" ? null : !isClaude(agent) || MODES.includes(v as any) ? String(v) : undefined;
// A project's defaults from a request body, {agent: {config id: value, mode}}, stored as JSON (null when none is set);
// a string when a value is not taken.
function defaultsIn(v: unknown): string | null | { error: string } {
  const out: Record<string, Record<string, string>> = {};
  for (const [agent, d] of Object.entries(v && typeof v === "object" && !Array.isArray(v) ? v : {})) {
    if (!isKnown(agent)) return { error: `no agent ${agent}` };
    const config: Record<string, string> = configIn(d);
    const mode = modeIn((d as any)?.mode, agent);
    if (mode === undefined) return { error: `mode must be one of ${MODES.join(", ")}` };
    if (mode && !(isClaude(agent) && mode === "auto")) config.mode = mode; // auto is Claude's default anyway
    if (Object.keys(config).length) out[agent] = config;
  }
  return Object.keys(out).length ? JSON.stringify(out) : null;
}
const agentIn = (v: unknown): string | null | undefined => (v == null || v === "" ? null : isKnown(String(v)) ? String(v) : undefined);
// A custom agent's arguments (a list, or one string split on spaces) and environment ({name: value}, or NAME=value lines).
const agentArgs = (v: unknown): string[] => Array.isArray(v) ? v.map(String) : String(v ?? "").split(/\s+/).filter(Boolean);
const agentEnv = (v: unknown): Record<string, string> => v && typeof v === "object" && !Array.isArray(v)
  ? Object.fromEntries(Object.entries(v).map(([k, x]) => [k, String(x)]))
  : Object.fromEntries(String(v ?? "").split("\n").map((l) => l.trim()).filter((l) => l.includes("=")).map((l) => [l.slice(0, l.indexOf("=")).trim(), l.slice(l.indexOf("=") + 1)]));
const httpError = (status: number, msg: string) => Object.assign(new Error(msg), { status });

function todoOr404(id: number) {
  const t = todoRow(id);
  if (!t) throw httpError(404, "no such TODO");
  return t;
}
// A config or mode change reaches a live agent process at once (applyConfig). The change is stored either way; what
// the agent did not take (a mode the model lacks) comes back as a warning, and the next turn tries again.
async function applyNow(todoId: number, reset?: string) {
  const a = liveAgent(todoId);
  if (!a) return json({});
  await applyConfig(todoId, a, reset);
  const warning = a.configError || null;
  a.configError = "";
  if (warning) addEvent(todoId, "error", "app", warning);
  return json({ warning });
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
function uploadsIn(v: unknown): Upload[] {
  if (v == null) return [];
  if (!Array.isArray(v) || !v.every((u) => typeof u?.name === "string" && typeof u?.data === "string"))
    throw httpError(400, "attachments must be a list of {name, data}");
  return v;
}

// ---------- worktree and project files (items 101-110) ----------

// A request names a root and a path relative to it. The real path must lie inside the project's local folder or its
// worktree root (item 110), so the worktree skill's symlinks into the main folder open and nothing else does.
function rootDir(t: Todo, root: string) {
  const base = root === "worktree" ? t.worktree_path : root === "project" ? projectRow(t.project_id)!.local_path : null;
  if (!base || !existsSync(base)) throw httpError(404, root === "worktree" ? "this TODO has no worktree on disk" : "unknown root");
  return base;
}
function fsTarget(t: Todo, root: string, rel: string, write = false) {
  const p = projectRow(t.project_id)!;
  const base = rootDir(t, root);
  if (typeof rel !== "string" || !rel || isAbsolute(rel)) throw httpError(400, "path must be relative to the root");
  const lexical = resolve(base, rel);
  if (!isInside(lexical, resolve(base))) throw httpError(403, `${rel} is outside the ${root}`);
  // A link whose target is missing would make a write create that target, wherever it points.
  if (!existsSync(lexical)) { let link = false; try { link = lstatSync(lexical).isSymbolicLink(); } catch {} if (link) throw httpError(403, `${rel} is a link to a missing file`); }
  const real = existsSync(lexical) ? realpathSync(lexical) : join(realpathSync(dirname(lexical)), basename(lexical));
  const allowed = [realpathSync(p.local_path), realpathSync(worktreeRoot(p.local_path))];
  if (!allowed.some((d) => isInside(real, d))) throw httpError(403, `${rel} points outside the project`);
  if (write && (rel.split("/").includes(".git") || real.split("/").includes(".git"))) throw httpError(403, "writes inside .git/ are refused");
  return { base, real };
}
// The base the marks and Diff compare with: the merge base of HEAD with origin/HEAD, else HEAD.
async function diffBase(dir: string) {
  const mb = await sh(["git", "-C", dir, "merge-base", "HEAD", "origin/HEAD"]);
  if (mb.ok) return { sha: mb.out, label: "merge-base" };
  const head = await sh(["git", "-C", dir, "rev-parse", "HEAD"]);
  return head.ok ? { sha: head.out, label: "HEAD" } : null;
}
const IMAGE_EXT = /\.(png|jpe?g|gif|webp|avif|bmp|ico|svg)$/i;
const MAX_TEXT = 2 * 1024 * 1024;
// ponytail: a whole read per request; fine for source files, the 2 MB cut keeps big files out of the JSON.
function fileFacts(real: string) {
  const st = statSync(real);
  if (!st.isFile()) throw httpError(400, "not a file");
  const image = IMAGE_EXT.test(real) && !/\.svg$/i.test(real);
  if (image || st.size > MAX_TEXT) return { size: st.size, mtime: st.mtimeMs, image, large: !image, binary: false, content: null };
  const buf = readFileSync(real);
  const binary = buf.subarray(0, 8192).includes(0);
  return { size: st.size, mtime: st.mtimeMs, image, large: false, binary, content: binary ? null : buf.toString("utf8") };
}

// ---------- terminals (items 111-120) ----------

// Kept in memory only: a terminal ends with its tab, its TODO's removal or the app (item 115).
type Term = {
  id: number; todoId: number; kind: "shell" | "claude"; root: "worktree" | "project"; cwd: string; no: number; message: string;
  title: string | null; proc: Subprocess | null; exited: boolean; code: number | null; signal: string | null;
  secret: string | null;   // a Claude terminal's grant for the app tools (P6), revoked when its process ends
  buf: Uint8Array[]; bufLen: number; sockets: Set<ServerWebSocket<{ tid: number }>>; cols: number; rows: number; fresh: boolean;
};
const terms = new Map<number, Term>();
const termNos = new Map<string, number>();   // "todo:kind" -> last number; numbers only count up (Shell 1, Shell 2, ...)
let termSeq = 0;
const TERM_BUF = 256 * 1024;   // scrollback replayed when a page attaches
// Bun's PTY runs on Windows (ConPTY) since 1.4; 1.4.2 is the version it was checked with.
const TERM_MIN = process.platform === "win32" ? "1.4.2" : "1.3.5";
export const TERM_UNSUPPORTED = !versionAtLeast(Bun.version, TERM_MIN) ? `Terminals need Bun ${TERM_MIN} or newer${process.platform === "win32" ? " on Windows" : ""}. This server runs Bun ${Bun.version}.` : null;
// The shell of a new terminal: a login shell on macOS and Linux; PowerShell (else cmd.exe) on Windows, which has none.
const SHELL = process.env.SWITCHBOARD_SHELL || (process.platform === "win32"
  ? Bun.which("pwsh") || Bun.which("powershell") || process.env.COMSPEC || "cmd.exe"
  : process.env.SHELL || "/bin/zsh");
const LOGIN_SHELL = process.platform !== "win32";
const termOut = (x: Term) => ({ id: x.id, kind: x.kind, root: x.root, cwd: x.cwd, no: x.no, title: x.title, exited: x.exited, code: x.code, signal: x.signal });
const termsOf = (todoId: number) => [...terms.values()].filter((x) => x.todoId === todoId);
const enc8 = new TextEncoder(), dec8 = new TextDecoder();

// Item 114: the TODO as a Claude terminal reads it, rewritten at each launch.
async function writeContext(t: Todo) {
  const [s] = await summaries("t.id = ?", [t.id]);
  const msgs = db.query("SELECT author, body, committed_at AS at, session_no FROM messages WHERE todo_id = ? ORDER BY id").all(t.id) as any[];
  const dir = artifactDir(projectRow(t.project_id)!.slug, t.id);
  const g = s.git, pr = s.pr;
  const text = contextFile({
    title: s.title, project: s.project_name, status: s.status, first: msgs[0]?.body ?? "", worktree: s.worktree_path, branch: s.branch,
    git: g ? [g.commits != null && `${g.commits} commits`, g.changed && `${g.changed} changed`, g.untracked && `${g.untracked} untracked`,
      g.push === "none" ? "not pushed" : g.push === "unpushed" ? `${g.unpushed} unpushed` : g.push].filter(Boolean).join(" · ") : null,
    pr: pr ? `#${pr.number} ${pr.state}${pr.draft ? " (draft)" : ""} · ${pr.url}` : s.pr_number ? `#${s.pr_number}` : null,
    sessions: sessionsOf(t.id), messages: msgs, files: filesOf(t.id).map((f) => f.path),
  });
  mkdirSync(join(dir, "terminal"), { recursive: true });
  writeFileSync(join(dir, "terminal", "context.md"), text);
}

function termSend(x: Term, data: Uint8Array | string) { for (const ws of x.sockets) ws.send(data); }
async function spawnTerm(x: Term) {
  const t = todoRow(x.todoId)!;
  const dir = artifactDir(projectRow(t.project_id)!.slug, t.id);
  mkdirSync(dir, { recursive: true });
  let cmd: string[], secret: string | null = null;
  if (x.kind === "claude") {
    const claude = onPath("claude", []);
    if (!claude) throw httpError(400, "claude is not on PATH");
    await writeContext(t);
    if (terms.get(x.id) !== x || x.proc) return;   // closed, or restarted twice, while the context file was written
    // P6: the app tools, granted to this process and named in files rather than argv. The header is a ${VAR} reference
    // that Claude Code expands from its environment, so the secret is in neither the file nor the command line.
    const g = grant({ kind: "worker", todoId: t.id, dir, onPost: () => {}, via: `Claude ${x.no}` }, () => "${SWITCHBOARD_MCP_SECRET}");
    x.secret = secret = g.secret;
    const mcp = join(dir, "terminal", "mcp.json"), contract = join(dir, "terminal", "contract.md");
    writeFileSync(mcp, JSON.stringify({ mcpServers: { app: { type: "http", url: g.server.url, headers: { "X-Switchboard-Session": "${SWITCHBOARD_MCP_SECRET}" } } } }, null, 1));
    writeFileSync(contract, TERMINAL_CONTRACT.replaceAll("{artifact_dir}", dir));
    // Run directly with its arguments, not through a shell, so the first message needs no quoting.
    // `--add-dir` and the other list flags take every argument after them, so `--` ends them before the first message.
    cmd = [...claude, "--add-dir", dir, "--mcp-config", mcp, "--append-system-prompt-file", contract, "--allowedTools", WORKER_TOOLS.filter((n) => !n.endsWith("__suggest_follow_ups")).join(","),
      ...(x.message ? ["--", x.message] : [])];
  } else cmd = LOGIN_SHELL ? [SHELL, "-l"] : [SHELL];
  const env: Record<string, string | undefined> = { ...process.env, TERM: "xterm-256color", COLORTERM: "truecolor", SWITCHBOARD_TODO_ID: String(t.id), SWITCHBOARD_FILES: dir };
  if (secret) env.SWITCHBOARD_MCP_SECRET = secret;
  delete env.SWITCHBOARD_PARENT;   // a `bun server.ts` started in the terminal must not take this server's stdin rule
  Object.assign(x, { exited: false, code: null, signal: null, buf: [], bufLen: 0, title: null });
  let proc: Subprocess;
  try { proc = Bun.spawn(cmd, {
    cwd: x.cwd, env,
    terminal: {
      cols: x.cols, rows: x.rows,
      data: (_term, data) => {
        x.buf.push(data); x.bufLen += data.length;
        while (x.bufLen > TERM_BUF && x.buf.length > 1) x.bufLen -= x.buf.shift()!.length;
        const title = parseOscTitle(dec8.decode(data));
        if (title !== null && title !== x.title) { x.title = title; emit({ type: "terminals", id: x.todoId }); }
        termSend(x, data);
      },
    },
  }); } catch (e: any) {
    // e.g. the worktree was deleted after a merge: the tab shows the failure instead of a terminal with no process
    x.exited = true;
    endGrant(x);
    emit({ type: "terminals", id: x.todoId });
    throw httpError(400, `could not start ${cmd[0]}: ${e.message}`);
  }
  x.proc = proc;
  emit({ type: "terminals", id: x.todoId });
  // The exit code comes from the process; the PTY's own exit callback only reports that the stream closed.
  proc.exited.then((code) => {
    if (x.proc !== proc) return;
    x.exited = true;
    endGrant(x);
    x.signal = proc.signalCode ?? null;
    x.code = x.signal ? null : code;
    termSend(x, JSON.stringify({ type: "exit", code: x.code, signal: x.signal }));
    emit({ type: "terminals", id: x.todoId });
  });
}
function endGrant(x: Term) {
  if (x.secret) revoke(x.secret);
  x.secret = null;
}
function killTerm(x: Term) {
  const p = x.proc;
  x.proc = null;
  endGrant(x);
  // Windows has no SIGHUP (Bun throws on it): the default kill ends the process there.
  try { process.platform === "win32" ? p?.kill() : p?.kill("SIGHUP"); p?.terminal?.close(); } catch {}
}
export function closeTerms(todoId: number | null) {
  for (const x of [...terms.values()]) if (todoId === null || x.todoId === todoId) {
    killTerm(x);
    for (const ws of x.sockets) ws.close();
    terms.delete(x.id);
  }
  if (todoId !== null) emit({ type: "terminals", id: todoId });
}
const termWs = {
  open(ws: ServerWebSocket<{ tid: number }>) {
    const x = terms.get(ws.data.tid);
    if (!x) return ws.close(4404, "no such terminal");
    x.sockets.add(ws);
    // Reset, then the recent output, then live output; the first resize makes full-screen programs redraw.
    ws.send(enc8.encode("\x1bc"));
    for (const b of x.buf) ws.send(b);
    if (x.exited) ws.send(JSON.stringify({ type: "exit", code: x.code, signal: x.signal }));
    x.fresh = true;
  },
  message(ws: ServerWebSocket<{ tid: number }>, msg: string | Buffer) {
    const x = terms.get(ws.data.tid);
    if (!x?.proc || x.exited) return;
    if (typeof msg !== "string") return void x.proc.terminal?.write(msg);
    const c = JSON.parse(msg);
    if (c.type !== "resize") return;
    const cols = Math.max(2, Math.min(500, c.cols | 0)), rows = Math.max(2, Math.min(300, c.rows | 0));
    // A page that attaches with the same size would not send SIGWINCH, so vim or Claude Code would not redraw.
    if (x.fresh && cols === x.cols && rows === x.rows) x.proc.terminal?.resize(cols, rows - 1);
    x.fresh = false;
    x.cols = cols; x.rows = rows;
    x.proc.terminal?.resize(cols, rows);
  },
  close(ws: ServerWebSocket<{ tid: number }>) { terms.get(ws.data.tid)?.sockets.delete(ws); },
};

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
    // The Waiting lane (laneOf): the status, or a running agent waiting for a permission answer.
    const w = db.query("SELECT COUNT(*) AS n FROM todos WHERE (status = 'waiting' OR (status = 'running' AND permission IS NOT NULL)) AND removed_at IS NULL").get() as any;
    const last = getMeta("last_project_id");
    // P4: the config options last seen from any session, for Settings and a TODO without a session yet.
    // P5: the agents, and per agent its last seen config options.
    return json({ last_project_id: last ? Number(last) : null, contract: CONTRACT, waiting_count: w.n, agents: agentList(), agent_config: agentConfigs(),
      shell: basename(SHELL).replace(/\.exe$/i, ""), login_shell: LOGIN_SHELL,
      background_wait_hours: Number(getMeta("background_wait_hours")) || DEFAULT_BACKGROUND_HOURS });
  }
  // Settings › Runs (item 136): how long an agent may wait for its background tasks between turns.
  if (path === "/api/settings" && method === "PUT") {
    const h = Number(body.background_wait_hours);
    if (!(h >= 0.25 && h <= 48)) return fail("background tasks: between 0.25 and 48 hours");
    setMeta("background_wait_hours", String(h));
    return json({ background_wait_hours: h });
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
    const defaults = defaultsIn(body.defaults), agent = agentIn(body.default_agent);
    if (defaults && typeof defaults === "object") return fail(defaults.error);
    if (agent === undefined) return fail(`no agent ${body.default_agent}`);
    const id = Number(db.run(
      "INSERT INTO projects (name, slug, local_path, agent_prompt, created_at, defaults, default_agent) VALUES (?, ?, ?, ?, ?, ?, ?)",
      [name, uniqueSlug(slugify(name), taken), localPath, String(body.agent_prompt ?? ""), now(), defaults, agent === DEFAULT_AGENT ? null : agent],
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
    const defaults = body.defaults === undefined ? p.defaults : defaultsIn(body.defaults);
    if (defaults && typeof defaults === "object") return fail(defaults.error);
    const agent = body.default_agent === undefined ? p.default_agent : agentIn(body.default_agent);
    if (agent === undefined) return fail(`no agent ${body.default_agent}`);
    db.run("UPDATE projects SET name = ?, local_path = ?, agent_prompt = ?, defaults = ?, default_agent = ? WHERE id = ?",
      [name, localPath, body.agent_prompt === undefined ? p.agent_prompt : String(body.agent_prompt), defaults, agent === DEFAULT_AGENT ? null : agent, p.id]);
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
    const agent = agentIn(body.agent);
    if (agent === undefined) return fail(`no agent ${body.agent}`);
    const id = createTodo(Number(body.project_id), instruction, "user", uploadsIn(body.attachments), agent);
    setMeta("last_project_id", String(Number(body.project_id)));
    if (body.start === true) start(id);   // Add & start (item 92): one request, so the TODO never sits in the backlog
    return json((await summaries("t.id = ?", [id]))[0]);
  }

  // Items 94-97: the first instruction and the project can change until the TODO starts.
  if ((m = path.match(/^\/api\/todos\/(\d+)\/edit$/)) && method === "POST") {
    const t = liveTodo(Number(m[1]));
    if (t.status !== "not_started") return fail("this TODO has started; its first instruction and project are fixed now", 409);
    const first = db.query("SELECT id, body FROM messages WHERE todo_id = ? ORDER BY id LIMIT 1").get(t.id) as { id: number; body: string };
    const instruction = body.instruction === undefined ? null : String(body.instruction).trim();
    if (instruction === "") return fail("the first instruction is required");
    const to = body.project_id === undefined ? null : projectRow(Number(body.project_id));
    if (body.project_id !== undefined && !to) return fail("choose a project");
    const from = projectRow(t.project_id)!;
    const agent = body.agent === undefined ? null : agentIn(body.agent);
    if (agent === undefined) return fail(`no agent ${body.agent}`);
    db.transaction(() => {
      // P5: the agent of the first session, like the project, until the start.
      if (agent && agent !== (t.agent || DEFAULT_AGENT)) {
        setTodo(t.id, { agent, mode: null, overrides: null, config: null });
        addEvent(t.id, "edited", "user", `agent ${agentName(t.agent)} → ${agentName(agent)}`);
      }
      if (instruction !== null && instruction !== first.body) {
        db.run("UPDATE messages SET body = ? WHERE id = ?", [instruction, first.id]);
        // The title is the first instruction until an agent sets one (item 7).
        setTodo(t.id, t.title === first.body ? { title: instruction } : {});
        addEvent(t.id, "edited", "user", "first instruction");
      }
      if (to && to.id !== from.id) {
        if (termsOf(t.id).some((x) => !x.exited)) throw httpError(409, "close this TODO's terminals first; they run in the old project's folders");
        // Nothing else is stored per project before the start: the artifact folder holds only the attachments.
        // HOME is a real path, so the stored paths start with these folders exactly.
        const a = artifactDir(from.slug, t.id), b = artifactDir(to.slug, t.id);
        if (existsSync(a)) { mkdirSync(join(b, ".."), { recursive: true }); renameSync(a, b); }
        for (const tbl of ["attachments", "files"])
          db.run(`UPDATE ${tbl} SET path = ? || substr(path, ?) WHERE substr(path, 1, ?) = ?`, [b + "/", a.length + 2, a.length + 1, a + "/"]);
        setTodo(t.id, { project_id: to.id });
        addEvent(t.id, "edited", "user", `project ${from.name} → ${to.name}\nMoved data/${from.slug}/${t.id}/ → data/${to.slug}/${t.id}/\nAttachment paths rewritten.`);
        emit({ type: "projects" });
      }
    })();
    emit({ type: "todos" });
    return json((await summaries("t.id = ?", [t.id]))[0]);
  }

  if ((m = path.match(/^\/api\/todos\/(\d+)\/files\/(\d+)(?:\/(reveal))?$/))) {
    const id = Number(m[1]);
    const t = todoOr404(id);
    const f = db.query("SELECT id, path FROM files WHERE id = ? AND todo_id = ?").get(Number(m[2]), id) as { id: number; path: string } | null;
    if (!f) return fail("no such file", 404);
    if (m[3] === "reveal") return method === "POST" ? reveal(f.path) : fail("not found", 404);
    if (method === "GET") {
      if (!existsSync(f.path)) return fail(`${f.path} no longer exists on disk`, 404);
      if (new URL(req.url).searchParams.has("raw")) return new Response(Bun.file(f.path), { headers: { "content-security-policy": "sandbox" } });
      return json({ id: f.id, name: basename(f.path), path: f.path, ...fileFacts(f.path) });
    }
    if (method === "PUT") {
      if (typeof body.content !== "string") return fail("content is required");
      if (t.status === "running") return json({ error: "running" }, 409);
      const mtime = existsSync(f.path) ? statSync(f.path).mtimeMs : null;
      if (!body.force && mtime !== body.base_mtime) return json({ error: "changed", mtime }, 409);
      writeFileSync(f.path, body.content); // the agent is not told; the user hands work back with a chat message
      addEvent(id, "file saved", "user", `${basename(f.path)} · agent not notified`);
      return json({ mtime: statSync(f.path).mtimeMs });
    }
  }

  if ((m = path.match(/^\/api\/todos\/(\d+)\/(tree|fs|fs\/raw|fs\/reveal|diff)$/))) {
    const t = todoOr404(Number(m[1])), what = m[2];
    const url = new URL(req.url), root = url.searchParams.get("root") ?? body?.root, rel = url.searchParams.get("path") ?? body?.path;
    if (what === "tree" && method === "GET") {
      const base = rootDir(t, String(root));
      const b = await diffBase(base);
      const [files, others, diff] = await Promise.all([
        sh(["git", "-C", base, "--no-optional-locks", "ls-files", "--cached", "--others", "--exclude-standard", "-z"]),
        sh(["git", "-C", base, "--no-optional-locks", "ls-files", "--others", "--exclude-standard", "-z"]),
        b ? sh(["git", "-C", base, "--no-optional-locks", "diff", "--relative", "--name-status", "-z", b.sha]) : Promise.resolve({ ok: true, out: "" }),
      ]);
      if (!files.ok) return fail(`git could not list ${base}`, 500);
      const marks = parseMarks(diff.ok ? diff.out : "", others.out);
      const paths = new Set(files.out.split("\0").filter(Boolean));
      // The worktree skill links .env or CLAUDE.local.md into the main folder; git ignores them, the tree shows them.
      // ponytail: only symlinks at the top of the root are found this way.
      const links: string[] = [];
      for (const n of readdirSync(base)) {
        try { if (lstatSync(join(base, n)).isSymbolicLink()) { links.push(n); paths.add(n); } } catch {}
      }
      return json({ paths: [...paths].filter((p) => marks[p] !== "D" || existsSync(join(base, p))), marks, links, base: b, dir: base });
    }
    if (what === "fs" && method === "GET") {
      const { real } = fsTarget(t, String(root), String(rel));
      if (!existsSync(real)) return fail(`${rel} no longer exists on disk`, 404);
      return json({ path: rel, abs: real, ...fileFacts(real) });
    }
    if (what === "fs/raw" && method === "GET") {
      const { real } = fsTarget(t, String(root), String(rel));
      if (!existsSync(real)) return fail("no such file", 404);
      return new Response(Bun.file(real), { headers: { "content-security-policy": "sandbox" } });
    }
    if (what === "fs/reveal" && method === "POST") return reveal(fsTarget(t, String(root), String(rel)).real);
    if (what === "fs" && method === "PUT") {
      const { real } = fsTarget(t, String(root), String(rel), true);
      if (typeof body.content !== "string") return fail("content is required");
      // Item 108: worktree files lock while the agent runs; the project folder has no lock, the mtime check guards it.
      if (root === "worktree" && t.status === "running") return json({ error: "running" }, 409);
      const mtime = existsSync(real) ? statSync(real).mtimeMs : null;
      if (!body.force && mtime !== body.base_mtime) return json({ error: "changed", mtime }, 409);
      writeFileSync(real, body.content);   // to the resolved file, so a symlink stays a symlink
      addEvent(t.id, "file saved", "user", `${root} · ${rel} · agent not notified`);
      return json({ mtime: statSync(real).mtimeMs });
    }
    if (what === "diff" && method === "GET") {
      const { base, real } = fsTarget(t, String(root), String(rel));
      const b = await diffBase(base);
      const top = await sh(["git", "-C", base, "rev-parse", "--show-toplevel"]);
      const inRepo = relative(top.ok ? realpathSync(top.out) : base, join(realpathSync(base), String(rel)));
      // Not through sh(): it trims, and a diff needs the exact text. A file added since the base has no old version.
      let old = "";
      if (b) {
        const pr = Bun.spawn(["git", "-C", base, "show", `${b.sha}:${inRepo}`], { stdout: "pipe", stderr: "ignore" });
        const [text, code] = await Promise.all([new Response(pr.stdout).text(), pr.exited]);
        if (code === 0) old = text;
      }
      return json({ base: old, base_sha: b?.sha ?? null, base_label: b?.label ?? null, current: existsSync(real) ? (fileFacts(real).content ?? "") : "" });
    }
  }

  if ((m = path.match(/^\/api\/todos\/(\d+)\/terminals(?:\/(\d+)\/(restart|close))?$/))) {
    const id = Number(m[1]);
    if (!m[2] && method === "GET") { todoOr404(id); return json({ terminals: termsOf(id).map(termOut), unsupported: TERM_UNSUPPORTED }); }
    if (!m[2] && method === "POST") {
      const t = liveTodo(id);
      if (TERM_UNSUPPORTED) return fail(TERM_UNSUPPORTED, 409);
      const kind = body.kind === "claude" ? "claude" : "shell";
      const root = body.root === "worktree" && t.worktree_path && existsSync(t.worktree_path) ? "worktree" : "project";
      const key = `${id}:${kind}`, no = (termNos.get(key) ?? 0) + 1;
      termNos.set(key, no);
      const x: Term = {
        id: ++termSeq, todoId: id, kind, root, cwd: root === "worktree" ? t.worktree_path! : projectRow(t.project_id)!.local_path, no,
        message: String(body.message ?? "").trim(), title: null, proc: null, exited: false, code: null, signal: null, secret: null,
        buf: [], bufLen: 0, sockets: new Set(), fresh: false,
        cols: Math.max(2, Math.min(500, Number(body.cols) || 100)), rows: Math.max(2, Math.min(300, Number(body.rows) || 30)),
      };
      terms.set(x.id, x);
      try { await spawnTerm(x); } catch (e) { terms.delete(x.id); throw e; }
      return json(termOut(x));
    }
    const x = terms.get(Number(m[2]));
    if (!x || x.todoId !== id) return fail("no such terminal", 404);
    if (m[3] === "close" && method === "POST") {
      killTerm(x); for (const ws of x.sockets) ws.close(); terms.delete(x.id);
      emit({ type: "terminals", id });
      return json({});
    }
    if (m[3] === "restart" && method === "POST") {
      killTerm(x);
      termSend(x, enc8.encode("\x1bc"));
      await spawnTerm(x);
      return json(termOut(x));
    }
  }

  if ((m = path.match(/^\/api\/attachments\/(\d+)$/)) && method === "GET") {
    const a = db.query("SELECT path FROM attachments WHERE id = ?").get(Number(m[1])) as { path: string } | null;
    if (!a || !existsSync(a.path)) return fail("no such attachment", 404);
    // sandbox: an uploaded SVG or HTML opened in a tab cannot run script against this API.
    return new Response(Bun.file(a.path), { headers: { "content-security-policy": "sandbox" } });
  }

  if ((m = path.match(/^\/api\/todos\/(\d+)$/)) && method === "GET") {
    const id = Number(m[1]);
    const [todo] = await summaries("t.id = ?", [id]);
    if (!todo) return fail("no such TODO", 404);
    registerDiskFiles(todoRow(id)!);
    const ids = db.query("SELECT id FROM messages WHERE todo_id = ? ORDER BY id").all(id) as { id: number }[];
    return json({
      todo,
      // P5: each session's agent, and how to resume it outside the app where that agent has a command for it.
      sessions: sessionsOf(id).map((s) => ({ ...s, agent: s.agent || DEFAULT_AGENT, agent_name: agentName(s.agent), resume: resumeHint(s.agent, s.session_id) })),
      artifact_dir: artifactDir(projectRow(todo.project_id)!.slug, id) + "/",
      messages: ids.map((r) => messageOut(r.id)),
      files: filesOf(id).map(fileOut),
      events: (db.query("SELECT id, kind, actor, detail, at, session_no, meta FROM events WHERE todo_id = ? ORDER BY id").all(id) as any[])
        .map((e) => ({ ...e, meta: e.meta ? JSON.parse(e.meta) : null })),
      draft: getDraft(`todo:${id}`),
    });
  }

  if ((m = path.match(/^\/api\/todos\/(\d+)\/draft$/)) && method === "PUT") {
    const id = todoOr404(Number(m[1])).id;
    setDraft(`todo:${id}`, String(body.body ?? ""));
    changed(id);
    return json({});
  }

  // P3: the TODO's own permission mode (null: the project's). A live agent process gets it at once, also mid-turn;
  // otherwise the next run applies it.
  if ((m = path.match(/^\/api\/todos\/(\d+)\/mode$/)) && method === "PUT") {
    const t = liveTodo(Number(m[1]));
    const mode = modeIn(body.mode, t.agent);
    if (mode === undefined) return fail(`mode must be one of ${MODES.join(", ")}, or empty for the project's`);
    setTodo(t.id, { mode }, false);
    addEvent(t.id, "mode", "user", mode ?? `project's (${modeOf({ mode, agent: t.agent }, projectRow(t.project_id)!) ?? "the agent's own"})`);
    return applyNow(t.id);
  }

  // P4: one of the TODO's own config values (model, effort, …; empty: the project's). Like the mode, a live agent
  // process gets it at once, also mid-turn; otherwise the next run applies it. Only options and values the agent has
  // offered are taken.
  if ((m = path.match(/^\/api\/todos\/(\d+)\/config$/)) && method === "PUT") {
    const t = liveTodo(Number(m[1]));
    const id = String(body.id ?? ""), value = body.value == null ? "" : String(body.value);
    const known = (JSON.parse(t.config ?? getMeta(metaKey("agent_config", t.agent || DEFAULT_AGENT)) ?? "[]") as ConfigOption[]).find((o) => o.id === id);
    if (!known || id === "mode") return fail(`no config option ${id || "(none)"}`);
    if (value && !known.options.some((o) => o.value === value)) return fail(`${known.name} has no value ${value}`);
    const overrides = configIn(t.overrides ? JSON.parse(t.overrides) : null);
    if (value) overrides[id] = value;
    else delete overrides[id];
    setTodo(t.id, { overrides: Object.keys(overrides).length ? JSON.stringify(overrides) : null }, false);
    addEvent(t.id, "config", "user", `${known.name}: ${value ? known.options.find((o) => o.value === value)!.name : "not set"}`);
    return applyNow(t.id, value ? undefined : id);
  }

  // P4: the slash commands the composer completes.
  if ((m = path.match(/^\/api\/todos\/(\d+)\/commands$/)) && method === "GET") return json(commandsOf(todoOr404(Number(m[1])).id));

  // P3: the user's answer to the open permission request.
  if ((m = path.match(/^\/api\/todos\/(\d+)\/permission$/)) && method === "POST") {
    const t = liveTodo(Number(m[1]));
    if (!answerPermission(t.id, String(body.optionId ?? ""))) return fail("no such open permission request; the agent may have moved on", 409);
    return json({});
  }

  if ((m = path.match(/^\/api\/todos\/(\d+)\/(start|messages|read|clear-attention|remove|restore|deliver|new-session|stop)$/)) && method === "POST") {
    const id = Number(m[1]);
    const action = m[2];

    // Item 72: ends the live session and starts the next with the hand-over header, the queued messages and this one.
    if (action === "new-session") {
      const t = liveTodo(id);
      if (t.status === "not_started") return fail("start the TODO first; its first session starts with Start", 409);
      if (isRunning(id)) return fail("the agent is running; a new session can start once it stops", 409);
      const agent = agentIn(body.agent);
      if (agent === undefined) return fail(`no agent ${body.agent}`);
      const text = String(body.body ?? "").trim();
      const uploads = uploadsIn(body.attachments);
      const mid = db.transaction(() => {
        const mid = addMessage(id, "user", text || "continue", "queued");
        saveAttachments(id, mid, uploads);
        return mid;
      })();
      setDraft(`todo:${id}`, "");
      newSession(id, agent ?? undefined);
      return json(messageOut(mid));
    }

    if (action === "start") {
      if (liveTodo(id).status !== "not_started") return fail("this TODO has already started", 409);
      start(id);
    } else if (action === "messages") {
      liveTodo(id);
      const text = String(body.body ?? "").trim();
      const uploads = uploadsIn(body.attachments);
      if (!text && !uploads.length) return fail("empty message");
      if (body.suggested === true) {
        if (!(JSON.parse(todoRow(id)!.suggestions ?? "[]") as string[]).includes(text)) return fail("That suggestion is gone", 409);
        addEvent(id, "suggestion picked", "user", text);   // before the run's launch events
      }
      const mid = commitMessage(id, "user", text, uploads);
      // A pill click sends its text; the draft in the composer stays.
      if (body.suggested !== true) setDraft(`todo:${id}`, "");
      return json(messageOut(mid));
    } else if (action === "read") {
      // Only an actual change is announced, so a page that marks read on every refetch cannot loop.
      const [s] = await summaries("t.id = ?", [todoOr404(id).id]);
      if (s.unread) setTodo(id, { last_read_at: now() }, false);
      else db.run("UPDATE todos SET last_read_at = ? WHERE id = ?", [now(), id]);
    } else if (action === "clear-attention") {
      const t = todoOr404(id);
      if (t.attention) {
        setTodo(id, { attention: 0, attention_by: null, status: t.status === "waiting" ? "idle" : t.status });
        addEvent(id, "attention cleared", "user");
      }
    } else if (action === "remove") {
      liveTodo(id);
      setTodo(id, { removed_at: now(), removed_by: "user", suggestions: null });
      addEvent(id, "removed", "user");
      // Like Esc in Claude Code; queued messages stay queued. The agent process ends with the run, or now when idle.
      if (isRunning(id)) interrupt(id);
      else closeAgent(id);
      closeTerms(id); // restoring does not bring them back (item 115)
      emit({ type: "todos" });
    } else if (action === "restore") {
      const t = todoOr404(id);
      if (!t.removed_at) return fail("this TODO is not removed", 409);
      if (t.session_id) {
        const folder = projectRow(t.project_id)!.local_path;
        const spec = specOf(t.agent || DEFAULT_AGENT);
        if (spec && existsSync(folder) && !(await sessionExists(spec, folder, t.session_id).catch(() => true)))
          return json({ warning: `The ${agentName(t.agent)} session ${t.session_id} no longer exists.`, detail: RESTORE_DETAIL }, 409);
        if (t.worktree_path && !existsSync(t.worktree_path))
          return json({ warning: `The worktree ${t.worktree_path} no longer exists.`, detail: RESTORE_DETAIL }, 409);
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
    } else if (action === "stop") {
      // Like Esc in Claude Code. Messages queued during the turn are answered right after.
      // Also ends the background tasks the agent waits for between turns (item 136).
      if (!isRunning(liveTodo(id).id) && !backgroundCount(id)) return fail("the agent is not running", 409);
      interrupt(id, "user");
    } else if (action === "deliver") {
      if (isRunning(liveTodo(id).id)) return fail("the agent is running; queued messages go out when it stops", 409);
      deliver(id, "deliver");
    }
    return json({});
  }

  // ----- agents (P5) -----

  if (path === "/api/agents" && method === "GET") return json(agentList());
  // A custom agent: an ACP agent started as `command args…` with `env` added (Settings › Agents).
  if (path === "/api/agents" && method === "POST") {
    const name = String(body.name ?? "").trim(), command = String(body.command ?? "").trim();
    if (!name || !command) return fail("name and command are required");
    const id = slugify(String(body.id ?? name));
    if (isKnown(id)) return fail(`an agent ${id} exists already`);
    const args = agentArgs(body.args), env = agentEnv(body.env);
    db.run("INSERT INTO agents (id, name, command, args, env, created_at) VALUES (?, ?, ?, ?, ?, ?)", [id, name, command, JSON.stringify(args), JSON.stringify(env), now()]);
    emit({ type: "agents" });
    return json(agentList().find((a) => a.id === id));
  }
  if ((m = path.match(/^\/api\/agents\/([\w-]+)$/)) && (method === "PUT" || method === "DELETE")) {
    const id = m[1]!;
    if (isBuiltin(id)) return fail("the built-in agents cannot be changed", 409);
    const r = db.query("SELECT * FROM agents WHERE id = ?").get(id) as any;
    if (!r) return fail("no such agent", 404);
    if (method === "DELETE") {
      const used = db.query("SELECT COUNT(*) AS n FROM todos WHERE agent = ? AND removed_at IS NULL").get(id) as { n: number };
      if (used.n) return fail(`${used.n} TODO${used.n > 1 ? "s run" : " runs"} on this agent`, 409);
      db.run("DELETE FROM agents WHERE id = ?", [id]);
      db.run("UPDATE projects SET default_agent = NULL WHERE default_agent = ?", [id]);
    } else {
      db.run("UPDATE agents SET name = ?, command = ?, args = ?, env = ? WHERE id = ?", [
        String(body.name ?? r.name).trim() || r.name, String(body.command ?? r.command).trim() || r.command,
        body.args === undefined ? r.args : JSON.stringify(agentArgs(body.args)), body.env === undefined ? r.env : JSON.stringify(agentEnv(body.env)), id]);
    }
    emit({ type: "agents" });
    return json({});
  }
  // What the agent says about itself: its version and its ways to log in (Settings › Agents).
  if ((m = path.match(/^\/api\/agents\/([\w-]+)\/describe$/)) && method === "GET") {
    const id = m[1]!, spec = specOf(id);
    if (!spec) return fail(`${agentName(id)} is not installed`, 409);
    try { return json(await describe(spec)); } catch (e: any) { return fail(`${agentName(id)} did not start: ${e?.message ?? e}`, 502); }
  }
  // Log in with one of the agent's methods. The agent does it itself (Codex opens the browser); this waits until it is done.
  if ((m = path.match(/^\/api\/agents\/([\w-]+)\/login$/)) && method === "POST") {
    const id = m[1]!, spec = specOf(id);
    if (!spec) return fail(`${agentName(id)} is not installed`, 409);
    try {
      if (!(await authenticate(spec, String(body.methodId ?? "")))) return fail("the login did not complete", 409);
    } catch (e: any) {
      return fail(`the login failed: ${e?.message ?? e}`, 502);
    }
    return json({});
  }

  if (path === "/api/gh/retry" && method === "POST") {
    ghLogins = null; // pick up a `gh auth login` done since the last try
    await refreshAllPrs();
    return json({});
  }

  // ----- operator -----

  if (path === "/api/operator" && method === "GET") {
    const since = getMeta("operator_since") ?? "";
    const messages = db.query("SELECT id, author, body, committed_at AS at, state FROM messages WHERE todo_id IS NULL AND committed_at >= ? ORDER BY id").all(since) as any[];
    // Item 127: each reply lists the tools the operator used since the message before it.
    const used = db.query("SELECT kind, detail, at FROM events WHERE todo_id IS NULL AND actor = 'operator' AND at >= ? ORDER BY id").all(since) as any[];
    let prev = since;
    for (const m of messages) {
      if (m.author === "operator") m.tools = used.filter((e) => e.at > prev && e.at <= m.at).map((e) => `${e.kind}(${e.detail ?? ""})`);
      prev = m.at;
    }
    return json({
      status: isRunning("operator") ? "running" : "idle",
      session_id: getMeta("operator_session_id") ?? null,
      ctx: getMeta("operator_ctx") ? JSON.parse(getMeta("operator_ctx")!) : null,
      messages,
      draft: getDraft("operator"),
    });
  }
  if (path === "/api/operator/messages" && method === "POST") {
    const text = String(body.body ?? "").trim();
    if (!text) return fail("empty message");
    const id = commitOperatorMessage(text);
    setDraft("operator", "");
    return json(db.query("SELECT id, author, body, committed_at AS at, state FROM messages WHERE id = ?").get(id));
  }
  if (path === "/api/operator/draft" && method === "PUT") {
    setDraft("operator", String(body.body ?? ""));
    changed(null);
    return json({});
  }
  if (path === "/api/operator/new-session" && method === "POST") {
    if (isRunning("operator")) return fail("the operator is running; wait for its reply", 409);
    // The next message starts a new session id. Older messages stay in the database but are hidden.
    setMeta("operator_session_id", null);
    setMeta("operator_ctx", null);
    setMeta("operator_since", now());
    addEvent(null, "new operator session", "user");
    return json({});
  }

  return fail("not found", 404);
}

// Item 120: a random secret per start. Every /api/ request must carry it, so a program that can reach the port but
// cannot read ~/.switchboard/token (another user, a container on the host network) gets no data and no shell.
const TOKEN = Buffer.from(crypto.getRandomValues(new Uint8Array(32))).toString("hex");
const TOKEN_FILE = join(HOME, "token");
const VENDOR = join(import.meta.dir, "vendor");

const httpServer = Bun.serve({
  hostname: HOST,
  port: PORT,
  idleTimeout: 0, // the default 10s would cut the SSE stream
  websocket: termWs,
  async fetch(req, server) {
    const url = new URL(req.url);
    // Blocks DNS-rebinding pages: the Host header must name this loopback server.
    if (![`${HOST}:${PORT}`, `localhost:${PORT}`].includes(req.headers.get("host") ?? "")) return fail("bad host", 403);
    // Cross-site forms can only send simple content types; requiring JSON forces a CORS preflight, which this server never answers.
    if (req.method !== "GET" && !req.headers.get("content-type")?.startsWith("application/json")) return fail("JSON body required", 415);
    // The page, the icon and the bundled libraries hold no data, so they stay open.
    if (url.pathname === "/") return new Response(Bun.file(join(import.meta.dir, "index.html")));
    if (url.pathname === "/favicon.svg") return new Response(Bun.file(join(import.meta.dir, "images/icons/flat/switchboard-flat-small.svg")));
    if (url.pathname.startsWith("/vendor/")) {
      const file = resolve(VENDOR, "." + decodeURIComponent(url.pathname.slice(7)));
      if (!isInside(file, VENDOR) || file.startsWith(join(VENDOR, "src")) || !existsSync(file)) return fail("not found", 404);
      return new Response(Bun.file(file), { headers: { "cache-control": "no-cache" } });
    }
    // The agents' route to the app's MCP tools: each agent session carries its own secret instead of the launch token.
    if (url.pathname === "/mcp/app") return handleMcp(req);
    if (!tokenOk(TOKEN, req.headers.get("x-switchboard-token"), url.searchParams.get("token"))) return fail("open the link the server printed at start", 401);
    // Browsers apply no CORS to WebSockets, so any website could open one; the Origin check stops websites, the token programs.
    const ws = url.pathname.match(/^\/api\/todos\/(\d+)\/terminals\/(\d+)\/ws$/);
    if (ws) {
      if (![`http://${HOST}:${PORT}`, `http://localhost:${PORT}`].includes(req.headers.get("origin") ?? "")) return fail("bad origin", 403);
      const x = terms.get(Number(ws[2]));
      if (!x || x.todoId !== Number(ws[1])) return fail("no such terminal", 404);
      return server.upgrade(req, { data: { tid: x.id } }) ? undefined : fail("websocket upgrade failed", 400);
    }
    try {
      return await api(req, url.pathname);
    } catch (e: any) {
      return fail(e.message ?? String(e), e.status ?? 500);
    }
  },
});
// Written only once the port is bound: a second copy that exits on EADDRINUSE never replaces the live server's token.
writeFileSync(TOKEN_FILE, TOKEN, { mode: 0o600 });
chmodSync(TOKEN_FILE, 0o600); // mode above applies only when the file is new
console.log(`switchboard on http://${HOST}:${PORT}/#token=${TOKEN} (data in ${HOME})`);

// A run cannot survive a restart. Queued messages stay queued until the user presses Deliver.
// Only after Bun.serve has bound the port: a second copy that fails with EADDRINUSE must not touch the live app's TODOs.
for (const t of db.query("SELECT id, attention, attention_by, permission FROM todos WHERE status = 'running'").all() as Todo[]) {
  // A permission request open at the crash died with its agent; the flag it raised drops with it.
  const attention = t.permission && t.attention_by === "permission" ? 0 : t.attention;
  setTodo(t.id, { status: statusAfterRun(attention), attention, attention_by: attention ? t.attention_by : null, permission: null });
  addEvent(t.id, "app restarted", "app");
}
refreshAllPrs();
setInterval(refreshAllPrs, 120_000);

// Interrupts every running agent so none keeps working unseen; a second Ctrl-C exits at once.
let stopping = false;
async function shutdown() {
  if (stopping) process.exit(1);
  stopping = true;
  console.log("stopping: interrupting running agents");
  // The port frees at once, so a window reopened during the 5 s below starts a new server instead of reading this
  // server's token and attaching to a server that is about to exit.
  httpServer.stop(true);
  closeTerms(null);
  await Promise.race([stopAll(), Bun.sleep(5000)]);
  try { if (readFileSync(TOKEN_FILE, "utf8") === TOKEN) unlinkSync(TOKEN_FILE); } catch {}
  process.exit(0);
}
process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);
// Started by electron/main.js: its stdin closes when the window app quits or crashes. Bun emits "end" only once stdin flows.
if (process.env.SWITCHBOARD_PARENT) process.stdin.on("end", shutdown).resume();
