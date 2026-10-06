// Pure helpers. No I/O here, so lib.test.ts can cover them (migrateDb takes the database, so the tests give it one in memory).
import type { Database } from "bun:sqlite";
import { basename, isAbsolute, relative } from "node:path";

export type Status = "not_started" | "running" | "waiting" | "idle";

// "Billing API!" -> "billing-api". Used once, when a project is created; the slug names its artifact folder.
export function slugify(name: string): string {
  return name.toLowerCase().normalize("NFKD").replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "") || "project";
}

// The slug itself if free, else slug-2, slug-3, ...
export function uniqueSlug(slug: string, taken: Set<string>): string {
  let s = slug;
  for (let n = 2; taken.has(s); n++) s = `${slug}-${n}`;
  return s;
}

// A turn's final text shown as a chat message: at most two lines and 280 characters,
// because one long paragraph is a single line too.
export function cutToTwoLines(text: string): string {
  const lines = text.trim().split("\n").map((l) => l.trim()).filter(Boolean);
  let out = lines.slice(0, 2).join("\n");
  if (out.length > 280) out = out.slice(0, 279).trimEnd();
  return out.length < lines.join("\n").length ? `${out}…` : out;
}

export const lineCount = (text: string) => text.trim().split("\n").filter((l) => l.trim()).length;

// The prompt for one turn: the worktree line (while one is registered), then every delivered message.
// With no messages, as on a restore with nothing queued and no draft, the prompt is "continue".
// The first turn of a later session starts with the hand-over header instead, which names the worktree itself.
export function deliveryPrompt(worktree: string | null, bodies: string[], header = ""): string {
  const text = bodies.map((b) => b.trim()).filter(Boolean).join("\n\n") || "continue";
  if (header) return `${header}\n\n${text}`;
  return worktree ? `Your worktree: ${worktree}. Work there.\n\n${text}` : text;
}

// Item 73: what a new session gets from the earlier ones, built from the TODO's data (no agent writes it).
// Lines without a value are left out.
export function handOver(o: {
  no: number; title: string; worktree: string | null; pr: number | null;
  last: { no: number; body: string } | null; files: { path: string; session_no: number | null }[];
}): string {
  return [
    `This TODO continues earlier work (session ${o.no}). Title: ${o.title}. Keep this title.`,
    o.worktree && `Your worktree: ${o.worktree}. Work there.`,
    o.pr && `Pull request: #${o.pr}.`,
    o.last && `Last message of session ${o.last.no}: ${o.last.body.replace(/\s*\n\s*/g, " ")}`,
    o.files.length && "Files from earlier sessions (open with Read what you need):\n"
      + o.files.map((f) => `${f.path} (${f.session_no ? `session ${f.session_no}` : "added outside a session"})`).join("\n"),
  ].filter(Boolean).join("\n");
}

// The context meter (item 123), updated from each SDK message of a run. Context = what went into the last model call
// of the main loop (input + cache read + cache write) plus what it wrote. result.usage is summed over the turn's calls,
// so it would overstate the context. ponytail: an assistant message's output_tokens can still be streaming; that is a
// few percent of `used`, so it is shown as is.
export type Ctx = { used: number; window: number | null; last_in: number; last_out: number; turns: number; model?: string };
export function usageStep(c: Ctx | null, m: any): Ctx | null {
  const cur: Ctx = c ?? { used: 0, window: null, last_in: 0, last_out: 0, turns: 0 };
  if (m?.type === "system" && m.subtype === "init") return { ...cur, model: m.model };
  if (m?.type === "assistant" && m.parent_tool_use_id == null && m.message?.usage) {
    const u = m.message.usage, input = (u.input_tokens ?? 0) + (u.cache_read_input_tokens ?? 0) + (u.cache_creation_input_tokens ?? 0);
    return { ...cur, last_in: input, used: input + (u.output_tokens ?? 0) };
  }
  if (m?.type === "system" && m.subtype === "compact_boundary") return { ...cur, used: m.compact_metadata?.post_tokens ?? cur.used };
  if (m?.type === "result") {
    const models = Object.entries(m.modelUsage ?? {}) as [string, any][];
    const main = models.find(([k]) => k === cur.model) ?? models.sort((a, b) => (b[1].contextWindow ?? 0) - (a[1].contextWindow ?? 0))[0];
    return { ...cur, last_out: m.usage?.output_tokens ?? cur.last_out, window: main?.[1].contextWindow ?? cur.window, turns: cur.turns + 1 };
  }
  return c;
}

// One message as the agent gets it: the text, then the absolute paths of the files the user attached to it.
export function withAttachments(body: string, paths: string[]): string {
  if (!paths.length) return body;
  return [body.trim(), `Attached files (open with Read):\n${paths.join("\n")}`].filter(Boolean).join("\n\n");
}

// "../My Shot (1).png" -> "My-Shot-1-.png": an uploaded file's name as one safe path segment, at most 120 characters
// (the end is kept, so the extension stays).
export function safeFileName(name: string): string {
  return basename(name).replace(/[^A-Za-z0-9._-]+/g, "-").slice(-120).replace(/^[.-]+/, "") || "file";
}

// What a committed message does, by the TODO's status:
// not started -> stored until start; running -> queued until the run ends; idle or waiting -> sent now.
export function deliveryAction(status: Status): "pending" | "queue" | "send" {
  return status === "not_started" ? "pending" : status === "running" ? "queue" : "send";
}

// A run that ended (normally, with an error, or by interrupt) leaves the TODO waiting while the flag is raised.
export const statusAfterRun = (attention: boolean | number): Status => (attention ? "waiting" : "idle");

// True when `path` is inside `dir` (not `dir` itself). Both must already be absolute and resolved.
export function isInside(path: string, dir: string): boolean {
  const rel = relative(dir, path);
  return rel !== "" && !rel.startsWith("..") && !isAbsolute(rel);
}

// The launch token a request carries (item 120): the X-Switchboard-Token header, or ?token= where a header cannot be
// set (EventSource, WebSocket, <img>). Compared in constant time.
export function tokenOk(token: string, header: string | null, query: string | null): boolean {
  const got = header ?? query ?? "";
  if (got.length !== token.length) return false;
  let diff = 0;
  for (let i = 0; i < got.length; i++) diff |= got.charCodeAt(i) ^ token.charCodeAt(i);
  return diff === 0;
}

// The interesting argument of a tool call: the Bash command, the file path, the search pattern...
export const toolArg = (input: any): string =>
  String(input?.command ?? input?.file_path ?? input?.pattern ?? input?.skill ?? input?.path ?? input?.url ?? "").slice(0, 300);

// "Bash git status", "Read /a/b.ts": one line per tool call for the events list.
export const toolLine = (name: string, input: any) => `${name} ${toolArg(input)}`.trim();

// ---------- git and GitHub (the card's git line and PR line) ----------

// `git status --porcelain=v2 --branch` -> the branch and how many files changed or are untracked.
// Changed counts ordinary (1), renamed (2) and unmerged (u) entries; "(detached)" means no branch.
export function parseGitStatus(out: string) {
  let branch: string | null = null, changed = 0, untracked = 0;
  for (const line of out.split("\n")) {
    if (line.startsWith("# branch.head ")) branch = line.slice(14) === "(detached)" ? null : line.slice(14);
    else if (/^[12u] /.test(line)) changed++;
    else if (line.startsWith("? ")) untracked++;
  }
  return { branch, changed, untracked };
}

// `git for-each-ref --format='%(refname) %(symref:short) %(ahead-behind:HEAD)' refs/remotes/origin/HEAD refs/remotes/origin/<branch>`:
//   refs/remotes/origin/HEAD origin/main 5 7       -> 5 behind main, 7 commits on the branch
//   refs/remotes/origin/<branch>  0 1              -> 1 commit not pushed; no such line -> the branch was never pushed
// No origin/HEAD line leaves base, behind and commits null (unknown, not zero).
export function parseRefCounts(out: string, branch: string | null) {
  let base: string | null = null, behind: number | null = null, commits: number | null = null, unpushed: number | null = null;
  for (const line of out.split("\n")) {
    const parts = line.trim().split(" ");
    if (parts.length < 3) continue;
    const theirs = Number(parts.at(-2)), mine = Number(parts.at(-1));
    if (parts[0] === "refs/remotes/origin/HEAD") [base, behind, commits] = [parts[1]!.replace(/^origin\//, "") || null, theirs, mine];
    else if (branch && parts[0] === `refs/remotes/origin/${branch}`) unpushed = mine;
  }
  const push = !branch ? null : unpushed === null ? "none" : unpushed ? "unpushed" : "pushed";
  return { base, behind, commits, push, unpushed: unpushed ?? 0 };
}

// "git@github.com:Hostaway/copilot-service.git" or "https://github.com/himmelreich-it/switchboard.git" -> owner and name.
export function parseGithubRemote(url: string) {
  const m = url.trim().match(/github\.com[:/]([^/\s]+)\/([^/\s]+?)(?:\.git)?\/?$/);
  return m ? { owner: m[1], name: m[2] } : null;
}

const PR_FIELDS = `fragment F on PullRequest { number url state isDraft mergeable mergeStateStatus reviewDecision baseRefName
  reviewThreads(first: 100) { nodes { isResolved } }
  commits(last: 1) { nodes { commit { statusCheckRollup { state contexts(first: 100) { nodes {
    __typename ... on CheckRun { name status conclusion } ... on StatusContext { context state } } } } } } } }`;

// One GraphQL query for a whole repository: alias b<i> finds the newest PR whose head is branch $b<i>,
// alias n<i> reads PR number $n<i> (registered with set_pr). Values go in as variables, never into the query text.
export function prQuery(branches: number, numbers: number) {
  const vars = ["$o: String!", "$n: String!"], parts: string[] = [];
  for (let i = 0; i < branches; i++) {
    vars.push(`$b${i}: String!`);
    parts.push(`b${i}: pullRequests(headRefName: $b${i}, first: 1, orderBy: {field: CREATED_AT, direction: DESC}) { nodes { ...F } }`);
  }
  for (let i = 0; i < numbers; i++) {
    vars.push(`$n${i}: Int!`);
    parts.push(`n${i}: pullRequest(number: $n${i}) { ...F }`);
  }
  return `query(${vars.join(", ")}) { repository(owner: $o, name: $n) { ${parts.join(" ")} } }\n${PR_FIELDS}`;
}

export type Pr = {
  number: number; url: string; state: "open" | "merged" | "closed"; draft: boolean; conflicts: boolean; mergeable: boolean;
  behind: boolean; base: string; review: "approved" | "changes" | "required" | null; threads: number;
  checks: { state: "failing" | "running" | "passing" | null; failing: string[]; running: number; passed: number; skipped: number };
};
const REVIEW: Record<string, Pr["review"]> = { APPROVED: "approved", CHANGES_REQUESTED: "changes", REVIEW_REQUIRED: "required" };
const ROLLUP: Record<string, Pr["checks"]["state"]> = { FAILURE: "failing", ERROR: "failing", PENDING: "running", EXPECTED: "running", SUCCESS: "passing" };

// One PullRequest node of prQuery() in the shape the card reads. The overall check state is GitHub's rollup;
// the counts and failing names come from the single checks. mergeable UNKNOWN (not computed yet) is neither.
export function prFromNode(n: any): Pr {
  const rollup = n.commits?.nodes?.[0]?.commit?.statusCheckRollup;
  const checks: Pr["checks"] = { state: ROLLUP[rollup?.state] ?? null, failing: [], running: 0, passed: 0, skipped: 0 };
  for (const c of rollup?.contexts?.nodes ?? []) {
    const result = c.__typename === "StatusContext" ? c.state : c.status === "COMPLETED" ? c.conclusion : "PENDING";
    if (result === "SUCCESS" || result === "NEUTRAL") checks.passed++;
    else if (result === "SKIPPED") checks.skipped++;
    else if (result === "PENDING" || result === "EXPECTED") checks.running++;
    else checks.failing.push(c.name ?? c.context);
  }
  return {
    number: n.number, url: n.url, state: String(n.state).toLowerCase() as Pr["state"], draft: !!n.isDraft,
    conflicts: n.mergeable === "CONFLICTING", mergeable: n.mergeable === "MERGEABLE", behind: n.mergeStateStatus === "BEHIND",
    base: n.baseRefName, review: REVIEW[n.reviewDecision] ?? null,
    threads: (n.reviewThreads?.nodes ?? []).filter((t: any) => !t.isResolved).length, checks,
  };
}

// How one `gh api graphql` call went. gh exits 1 on any GraphQL error but still prints the data, so a bad set_pr number
// (error on path repository.n0) keeps the other PRs. An error on the repository itself, or a rejected token (HTTP 401),
// means this account cannot see it, and the next account is tried.
export function ghOutcome(code: number | null, stdout: string, stderr: string):
  { repo: any } | { error: "no access" | "no network" | "unclear"; detail: string } {
  let j: any = null;
  try { j = JSON.parse(stdout); } catch {}
  if (j?.errors?.some((e: any) => e.path?.length === 1 && e.path[0] === "repository") || /HTTP 401/.test(stderr)) return { error: "no access", detail: "" };
  if (j?.data?.repository) return { repo: j.data.repository };
  const text = (stderr || stdout).trim();
  if (/dial tcp|error connecting to|internet connection|no such host|i\/o timeout|TLS handshake/i.test(text)) return { error: "no network", detail: text };
  return { error: "unclear", detail: `exit ${code ?? "killed"}${text ? " · " + text.split("\n")[0]!.slice(0, 160) : ""}` };
}

// ---------- files of a worktree or the project folder (items 102, 103) ----------

// `git diff --name-status -z <base>` (the merge base against the working tree, so committed and uncommitted
// changes) plus `git ls-files --others --exclude-standard -z` (untracked): one mark per path.
// A rename marks the new path added and the old one deleted.
export type Mark = "M" | "A" | "U" | "D";
export function parseMarks(nameStatusZ: string, untrackedZ: string): Record<string, Mark> {
  const marks: Record<string, Mark> = {};
  const f = nameStatusZ.split("\0");
  for (let i = 0; i < f.length - 1;) {
    const st = f[i++]!;
    if (!st) continue;
    if (st[0] === "R" || st[0] === "C") { const from = f[i++]!, to = f[i++]!; if (st[0] === "R") marks[from] = "D"; marks[to] = "A"; continue; }
    const path = f[i++]!;
    marks[path] = st[0] === "D" ? "D" : st[0] === "A" ? "A" : "M";
  }
  for (const p of untrackedZ.split("\0")) if (p) marks[p] = "U";
  return marks;
}

// ---------- terminals (items 111-118) ----------

// The title a program sets with OSC 0 or OSC 2 (ESC ] 0 ; title BEL, or ESC \\ as the end). Shells and Claude Code
// set one. The last one in the chunk wins; null when there is none.
// ponytail: a title split across two output chunks is missed; the next prompt sets it again.
export function parseOscTitle(chunk: string): string | null {
  let title: string | null = null;
  for (const m of chunk.matchAll(/\x1b\][02];([^\x07\x1b]*)(?:\x07|\x1b\\)/g)) title = m[1]!;
  return title;
}

// True when `version` ("1.3.14") is at least `min` ("1.3.5").
export function versionAtLeast(version: string, min: string): boolean {
  const a = version.split(".").map(Number), b = min.split(".").map(Number);
  for (let i = 0; i < 3; i++) if ((a[i] ?? 0) !== (b[i] ?? 0)) return (a[i] ?? 0) > (b[i] ?? 0);
  return true;
}

// Item 114: what a Claude terminal reads first, written to <artifact folder>/terminal/context.md at each launch.
export function contextFile(c: {
  title: string; project: string; status: string; first: string; worktree: string | null; branch: string | null;
  git: string | null; pr: string | null; sessions: { no: number; session_id: string; started_at: string; ended_at: string | null }[];
  messages: { author: string; body: string; at: string; session_no: number | null }[]; files: string[];
}): string {
  const out: string[] = [
    `# Switchboard TODO: ${c.title}`, "",
    `- Project: ${c.project}`, `- Status: ${c.status === "not_started" ? "not started (in the backlog)" : c.status}`,
    ...[c.worktree && `- Worktree: ${c.worktree}`, c.branch && `- Branch: ${c.branch}`, c.git && `- Git: ${c.git}`, c.pr && `- Pull request: ${c.pr}`].filter((l): l is string => !!l),
    "", "## First instruction", "", c.first, "",
    "## Note", "",
    "The TODO's own agent may be working in the same worktree at the same time. Check with the user before you commit, switch branches or rewrite history.", "",
  ];
  if (c.files.length) out.push("## Files", "", ...c.files.map((f) => `- ${f}`), "");
  if (c.sessions.length) out.push("## Sessions", "", ...c.sessions.map((s) => `- Session ${s.no}: ${s.session_id} (${s.started_at}${s.ended_at ? ` to ${s.ended_at}` : ", live"}). Its full transcript is under ~/.claude/projects/.`), "");
  out.push("## Chat", "");
  const groups = c.sessions.length ? c.sessions.map((s) => s.no) : [null];
  for (const no of groups) {
    const msgs = c.messages.filter((m) => no === null || (m.session_no ?? c.sessions.at(-1)!.no) === no);
    if (no !== null && c.sessions.length > 1) out.push(`### Session ${no}`, "");
    for (const m of msgs) out.push(`**${m.author}** (${m.at}): ${m.body}`, "");
  }
  return out.join("\n");
}

// ---------- schema ----------

// The whole schema plus its migrations. Every migration only adds, so an older build still reads the database.
export function migrateDb(db: Database) {
  const hadSessions = !!db.query("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'sessions'").get();
  db.exec(`
CREATE TABLE IF NOT EXISTS projects (
  id INTEGER PRIMARY KEY, name TEXT NOT NULL, slug TEXT NOT NULL UNIQUE, local_path TEXT NOT NULL,
  agent_prompt TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS todos (
  id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id), title TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('not_started','running','waiting','idle')), attention INTEGER NOT NULL DEFAULT 0,
  session_id TEXT, worktree_path TEXT, pr_number INTEGER, last_tool TEXT, last_read_at TEXT,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL, removed_at TEXT, removed_by TEXT);
CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY, todo_id INTEGER,
  author TEXT NOT NULL CHECK (author IN ('user','agent','user via operator','operator','app')), body TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('pending','queued','delivered')), file_id INTEGER, committed_at TEXT NOT NULL, delivered_at TEXT);
CREATE TABLE IF NOT EXISTS drafts (key TEXT PRIMARY KEY, body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS files (id INTEGER PRIMARY KEY, todo_id INTEGER NOT NULL, path TEXT NOT NULL UNIQUE);
CREATE TABLE IF NOT EXISTS attachments (id INTEGER PRIMARY KEY, message_id INTEGER NOT NULL, name TEXT NOT NULL, path TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS events (
  id INTEGER PRIMARY KEY, todo_id INTEGER, kind TEXT NOT NULL,
  actor TEXT NOT NULL CHECK (actor IN ('user','agent','app','operator')), detail TEXT, at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE IF NOT EXISTS sessions (
  id INTEGER PRIMARY KEY, todo_id INTEGER NOT NULL, no INTEGER NOT NULL, session_id TEXT NOT NULL,
  started_at TEXT NOT NULL, ended_at TEXT, UNIQUE (todo_id, no));
CREATE INDEX IF NOT EXISTS messages_todo ON messages (todo_id, state);
CREATE INDEX IF NOT EXISTS events_todo ON events (todo_id);
CREATE INDEX IF NOT EXISTS attachments_message ON attachments (message_id);
`);
  const addColumn = (table: string, col: string, type: string) => {
    if (!db.query(`SELECT 1 FROM pragma_table_info('${table}') WHERE name = ?`).get(col)) db.exec(`ALTER TABLE ${table} ADD COLUMN ${col} ${type}`);
  };
  addColumn("todos", "pr_number", "INTEGER");   // databases made before set_pr existed
  addColumn("todos", "ctx", "TEXT");            // context meter of the live session (JSON, see usageStep)
  addColumn("messages", "session_no", "INTEGER");
  addColumn("events", "session_no", "INTEGER");
  addColumn("events", "meta", "TEXT");           // a result's duration and tool count (JSON)
  addColumn("files", "session_no", "INTEGER");
  // Databases from before sessions: every started TODO was one session, which becomes session 1.
  if (!hadSessions) db.transaction(() => {
    db.exec(`
INSERT INTO sessions (todo_id, no, session_id, started_at)
  SELECT t.id, 1, t.session_id, COALESCE((SELECT MIN(e.at) FROM events e WHERE e.todo_id = t.id AND e.kind = 'launch'), t.created_at)
  FROM todos t WHERE t.session_id IS NOT NULL;
UPDATE messages SET session_no = 1 WHERE state = 'delivered' AND todo_id IN (SELECT todo_id FROM sessions);
UPDATE events SET session_no = 1 WHERE todo_id IN (SELECT todo_id FROM sessions)
  AND at >= (SELECT s.started_at FROM sessions s WHERE s.todo_id = events.todo_id);
UPDATE files SET session_no = 1 WHERE todo_id IN (SELECT todo_id FROM sessions);
`);
  })();
}
