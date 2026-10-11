// Pure helpers. No I/O here, so lib.test.ts can cover them (migrateDb takes the database, so the tests give it one in memory).
import type { Database } from "bun:sqlite";
import { basename, dirname, isAbsolute, relative } from "node:path";

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

// The agent's suggested follow-ups (pills): one or two one-line instructions the user sends with a click.
export function followUpError(items: string[]): string | null {
  if (items.length < 1 || items.length > 2) return "Suggest one or two follow-ups.";
  for (const s of items) {
    if (!s.trim()) return "A follow-up is empty.";
    if (s.trim().includes("\n")) return "A follow-up is one line.";
    if (s.trim().length > 80) return "A follow-up is at most 80 characters.";
  }
  return null;
}

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

// The context meter (item 123), from the agent's ACP usage_update: `used` is the context of the last model call, `size`
// the model's window. Claude names the model in `_meta["_claude/model"]`. last_in and last_out are no longer reported.
export type Ctx = { used: number; window: number | null; last_in?: number; last_out?: number; turns: number; model?: string };
export function ctxStep(c: Ctx | null, u: any): Ctx | null {
  if (u?.sessionUpdate !== "usage_update") return c;
  // Only the main loop's own context: Claude marks other cycles (a background task's follow-up, a subagent) by origin.
  const origin = u._meta?.["_claude/origin"]?.kind;
  if (origin && origin !== "human" && origin !== "channel") return c;
  const cur: Ctx = c ?? { used: 0, window: null, turns: 0 };
  return { ...cur, used: u.used ?? cur.used, window: u.size ?? cur.window, model: u._meta?.["_claude/model"] ?? cur.model };
}

// ---------- background tasks (item 136) ----------

// The agent's background tasks still running, oldest first, kept from its ACP updates (spike 2026-10-09). A Bash call
// with run_in_background ends at once with `backgroundTaskId` in its result; TaskStop (KillShell before it) names the
// task it stopped. Each finished task gets a follow-up cycle of its own, ended by a usage_update of origin
// task-notification; that does not say which task finished, so the oldest is taken off.
export function backgroundStep(ids: string[], u: any): string[] {
  const cc = u?._meta?.claudeCode, res = cc?.toolResponse;
  if (u?.sessionUpdate === "tool_call_update" && res) {
    if (typeof res.backgroundTaskId === "string") return ids.includes(res.backgroundTaskId) ? ids : [...ids, res.backgroundTaskId];
    const stopped = res.task_id ?? res.shell_id;
    if ((cc.toolName === "TaskStop" || cc.toolName === "KillShell") && ids.includes(stopped)) return ids.filter((id) => id !== stopped);
  }
  return u?.sessionUpdate === "usage_update" && u._meta?.["_claude/origin"]?.kind === "task-notification" && ids.length ? ids.slice(1) : ids;
}

// Between turns the agent can work on its own, e.g. a background task's follow-up. Such a cycle starts with any of
// these updates (not a title or a command list) and ends with a usage_update whose origin is not the user's turn.
export const agentActivity = (u: any) =>
  ["agent_message_chunk", "agent_thought_chunk", "tool_call", "tool_call_update", "plan"].includes(u?.sessionUpdate) || cycleEnd(u)
  || (u?.sessionUpdate === "usage_update" && !u._meta?.["_claude/origin"]);
export function cycleEnd(u: any) {
  const kind = u?.sessionUpdate === "usage_update" && u._meta?.["_claude/origin"]?.kind;
  return !!kind && kind !== "human" && kind !== "channel";
}

// ---------- transcript (item 130) ----------

const cut = (text: string, max: number) => (text.length > max ? text.slice(0, max) + `\n… ${text.length - max} more characters` : text);

// One ACP tool call as a transcript row's meta, merged from its tool_call and tool_call_update notifications. Text
// output is cut to 8 KB; diffs are kept up to 200 KB per side, enough for the code an agent writes in one call.
export type ToolMeta = {
  toolCallId: string; name: string; title: string; kind: string; status: string; input: any;
  locations: { path: string; line?: number }[]; diffs: { path: string; oldText: string | null; newText: string }[]; output: string;
};
export function toolMeta(prev: ToolMeta | null, u: any): ToolMeta {
  const m: ToolMeta = prev ? { ...prev } : {
    toolCallId: u.toolCallId, name: u._meta?.claudeCode?.toolName ?? u.name ?? u.title ?? "tool", title: "", kind: "other", status: "pending",
    input: {}, locations: [], diffs: [], output: "",
  };
  if (u.title) m.title = u.title;
  if (u.kind) m.kind = u.kind;
  if (u.status) m.status = u.status;
  if (u.rawInput && typeof u.rawInput === "object" && Object.keys(u.rawInput).length) m.input = { ...m.input, ...u.rawInput };
  if (Array.isArray(u.locations) && u.locations.length) m.locations = u.locations.map((l: any) => ({ path: l.path, ...(l.line ? { line: l.line } : {}) }));
  if (Array.isArray(u.content)) {
    const diffs = u.content.filter((c: any) => c.type === "diff");
    if (diffs.length) m.diffs = diffs.map((d: any) => ({ path: d.path, oldText: d.oldText == null ? null : cut(d.oldText, 200_000), newText: cut(d.newText ?? "", 200_000) }));
    const text = u.content.filter((c: any) => c.type === "content" && c.content?.type === "text").map((c: any) => c.content.text).join("\n");
    // While the call runs the content is its description; its result replaces it.
    if (text && (u.status === "completed" || u.status === "failed" || !m.output)) m.output = cut(text, 8000);
  }
  if (typeof u.rawOutput === "string" && u.rawOutput && (u.status === "completed" || u.status === "failed")) m.output = cut(u.rawOutput, 8000);
  return m;
}

// The agent's plan (its todo list) as `done/total`, or null without one.
export type PlanEntry = { content: string; status: "pending" | "in_progress" | "completed"; priority?: string };
export const planCount = (entries: PlanEntry[] | null) =>
  entries?.length ? `${entries.filter((e) => e.status === "completed").length}/${entries.length}` : null;

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

// What a committed message does, by the TODO's status (item 20) and whether a turn runs (`busy`):
// not started -> stored until start; a turn running -> queued, and handed to the turn at its next tool boundary or sent
// when it ends; otherwise sent now. A TODO that shows
// running only because its agent waits for background tasks (item 136) is not busy: the agent can answer now.
export function deliveryAction(status: Status, busy: boolean): "pending" | "queue" | "send" {
  return status === "not_started" ? "pending" : busy ? "queue" : "send";
}

// Item 20: a message committed during a turn goes to it at a tool boundary, the way typing into Claude Code does: when a
// tool call ends and no other call of the same batch (the calls the agent made together) still runs. Steering at any
// other moment cuts off a tool call being written, or moves a running command to the background and can end the turn
// early (designs/acp/acp-spike.md).
export const atToolBoundary = (batch: (string | null | undefined)[]) =>
  batch.length > 0 && batch.every((s) => s === "completed" || s === "failed");

// A run that ended (normally, with an error, or by interrupt) leaves the TODO waiting while the flag is raised.
export const statusAfterRun = (attention: boolean | number): Status => (attention ? "waiting" : "idle");

// ---------- permissions (P3) ----------

// The permission modes a project or TODO can run in: Claude's `mode` config option (spike finding 9), `auto` first
// because it is the default. The agent itself may not offer all of them (runner.ts reports when one cannot be set).
export const MODES = ["auto", "default", "acceptEdits", "plan", "bypassPermissions"] as const;

// An open session/request_permission, as stored in todos.permission and shown on the card and in the chat: the tool
// call's name, title, kind, input and diffs (as in a transcript row), plus the agent's options. One per TODO at most,
// because the agent blocks until it is answered.
export type Permission = {
  id: string; name: string; title: string; kind: string; input: any; diffs: ToolMeta["diffs"];
  options: { optionId: string; name: string; kind: string }[]; at: string;
};
export function permissionOf(r: { toolCall: any; options: any[] }, at: string): Permission {
  const m = toolMeta(null, r.toolCall ?? {});
  return {
    id: m.toolCallId, name: m.name, title: m.title, kind: m.kind, input: m.input, diffs: m.diffs, at,
    options: (r.options ?? []).map((o) => ({ optionId: String(o.optionId), name: String(o.name ?? o.optionId), kind: String(o.kind ?? "") })),
  };
}

// The board lane: a running TODO with an open permission request needs the user now, so it shows in Waiting.
// Otherwise the lane is the status. The tab title and ⌘J count lanes, not statuses.
export const laneOf = (t: { status: Status; permission: Permission | null }): Status =>
  t.status === "running" && t.permission ? "waiting" : t.status;

// Answering a request clears the attention flag only when the request raised it (`attention_by`); a flag the agent
// or a denial raised stays until they or the user clear it.
export const attentionAfterAnswer = (by: string | null): 0 | 1 => (by === "permission" ? 0 : 1);

// ---------- config options (P4) ----------

// A session config option as the UI shows it: the agent's option with its choices flattened (ACP allows groups).
export type ConfigOption = { id: string; name: string; category: string | null; value: string; options: { value: string; name: string }[] };
export function configView(raw: any[]): ConfigOption[] {
  return (raw ?? []).filter((o) => o?.type === "select" && o.id).map((o) => ({
    id: String(o.id), name: String(o.name ?? o.id), category: o.category ?? null, value: String(o.currentValue ?? ""),
    options: (o.options ?? []).flatMap((x: any) => x.options ?? [x]).map((x: any) => ({ value: String(x.value), name: String(x.name ?? x.value) })),
  }));
}

// The options a user sets per project and per TODO: model, effort (thought_level) and the like (fast, model_config).
// The permission mode has its own setting (P3); options without a category (Claude's `agent`) are left to the agent.
export const CONFIG_CATEGORIES = ["model", "thought_level", "model_config"];

// What to set for the wanted values ({config id: value}, the project's defaults under the TODO's own): the changes
// against the agent's current options, and the values it does not offer (effort on a model without it). `models`
// picks the model-category options only, or every other one: a model switch rewrites the others (P4 probe), so
// runner.ts sets the model first and the rest against the options that switch left.
export function configChanges(current: ConfigOption[], wanted: Record<string, string>, models: boolean) {
  const set: [string, string][] = [], skipped: [string, string][] = [];
  for (const [id, value] of Object.entries(wanted)) {
    const o = current.find((c) => c.id === id);
    if (id === "mode" || (o?.category === "model") !== models) continue; // an option the agent lacks: the second pass
    if (!o || !o.options.some((x) => x.value === value)) skipped.push([id, value]);
    else if (o.value !== value) set.push([id, value]);
  }
  return { set, skipped };
}

// A {config id: value} object from a request body or a column: string values only, empty ones dropped.
export function configIn(v: unknown): Record<string, string> {
  if (!v || typeof v !== "object" || Array.isArray(v)) return {};
  return Object.fromEntries(Object.entries(v).filter(([k, x]) => k !== "mode" && typeof x === "string" && x !== "").map(([k, x]) => [k, x as string]));
}

// ---------- agents (P5) ----------

// A project's defaults per agent, {agent id: {config id: value, mode: permission mode}}, as stored in projects.defaults.
// Config ids and modes are the agent's own, so each agent keeps its own set.
export type Defaults = Record<string, Record<string, string>>;
export function defaultsOf(raw: string | null): Defaults {
  const d = raw ? JSON.parse(raw) : null;
  if (!d || typeof d !== "object" || Array.isArray(d)) return {};
  return Object.fromEntries(Object.entries(d).filter(([, v]) => v && typeof v === "object" && !Array.isArray(v))
    .map(([agent, v]) => [agent, Object.fromEntries(Object.entries(v as object).filter(([, x]) => typeof x === "string" && x !== ""))]));
}
// One agent's defaults: its config values (without the mode) and its permission mode.
export function defaultsFor(raw: string | null, agent: string): { config: Record<string, string>; mode: string | null } {
  const d = defaultsOf(raw)[agent] ?? {};
  return { config: configIn(d), mode: d.mode || null };
}

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

// The command that shows a file in the system file manager (item 106): selected in Finder or Explorer. Linux has no
// common way to select a file, so its folder opens. Explorer wants backslashes and the quoted path glued to /select,
// and the caller spawns it with windowsVerbatimArguments: Bun's own quoting ("/select,C:\x y\z") makes Explorer open
// Documents instead. Windows paths cannot contain a quote, so none needs escaping.
export function revealCommand(platform: string, file: string): string[] {
  if (platform === "darwin") return ["open", "-R", file];
  if (platform === "win32") return ["explorer.exe", `/select,"${file.replaceAll("/", "\\")}"`];
  return ["xdg-open", dirname(file)];
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
    `- Repo: ${c.project}`, `- Status: ${c.status === "not_started" ? "not started (in the backlog)" : c.status}`,
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

// The whole schema plus its migrations. Migrations add, except one: repo routing (801) made todos.project_id
// nullable, and a build from before it fails on a TODO without a repo.
export function migrateDb(db: Database) {
  db.exec(`
CREATE TABLE IF NOT EXISTS projects (
  id INTEGER PRIMARY KEY, name TEXT NOT NULL, slug TEXT NOT NULL UNIQUE, local_path TEXT NOT NULL,
  agent_prompt TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS todos (
  id INTEGER PRIMARY KEY, project_id INTEGER REFERENCES projects(id), title TEXT NOT NULL,
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
    if (db.query(`SELECT 1 FROM pragma_table_info('${table}') WHERE name = ?`).get(col)) return false;
    db.exec(`ALTER TABLE ${table} ADD COLUMN ${col} ${type}`);
    return true;
  };
  addColumn("todos", "pr_number", "INTEGER");   // databases made before set_pr existed
  addColumn("todos", "ctx", "TEXT");            // context meter of the live session (JSON, see ctxStep)
  addColumn("todos", "plan", "TEXT");           // the agent's plan in its last turn (JSON entries), for the card
  addColumn("messages", "session_no", "INTEGER");
  addColumn("events", "session_no", "INTEGER");
  addColumn("events", "meta", "TEXT");           // a result's duration and tool count (JSON)
  addColumn("files", "session_no", "INTEGER");
  addColumn("projects", "mode", "TEXT");         // permission mode for the project's agents (null: auto), P3
  addColumn("todos", "mode", "TEXT");            // a TODO's own permission mode (null: the project's)
  addColumn("todos", "permission", "TEXT");      // the open permission request (JSON Permission), while the agent waits
  addColumn("todos", "attention_by", "TEXT");    // who raised the attention flag: agent, app or permission
  addColumn("todos", "config", "TEXT");          // the session's config options as the agent last reported them (JSON ConfigOption[]), P4
  addColumn("todos", "overrides", "TEXT");       // the TODO's own config values, {config id: value} (JSON)
  addColumn("projects", "defaults", "TEXT");     // the project's config values per agent, {agent: {config id: value, mode}} (JSON, P5)
  addColumn("projects", "default_agent", "TEXT"); // the agent of the project's new TODOs (null: claude), P5
  addColumn("todos", "agent", "TEXT");           // the agent of the TODO's live session, or of its first one before the start (null: claude)
  addColumn("sessions", "agent", "TEXT");        // the agent each session ran on (null: claude)
  addColumn("messages", "via", "TEXT");          // the Claude terminal an agent message came from ("Claude 2"), P6; null: the TODO's agent
  addColumn("todos", "suggestions", "TEXT");     // the agent's suggested follow-ups (JSON string[]), cleared at each run start
  addColumn("todos", "run_started_at", "TEXT");  // when the current or last run started, for the running timers (v6)
  addColumn("todos", "routing", "TEXT");         // the routing step while it runs or asks the user (JSON Routing), 801
  // When the TODO's repo (or "no repo") was decided. A TODO made before routing got its project from the user.
  if (addColumn("todos", "routed_at", "TEXT")) db.run("UPDATE todos SET routed_at = created_at");
  addColumn("projects", "profile", "TEXT");      // what lives in the repo, written by a scan or the user, for routing
  addColumn("projects", "profile_at", "TEXT");   // when the last scan wrote the profile
  addColumn("projects", "profile_edited", "INTEGER"); // 1 once the user edited the profile; a Rescan asks first
  // A TODO has no repo before routing, or none at all: project_id becomes nullable. SQLite cannot drop NOT NULL, so the
  // table is rebuilt from its own CREATE statement (it has no indexes or triggers).
  if ((db.query("SELECT \"notnull\" AS n FROM pragma_table_info('todos') WHERE name = 'project_id'").get() as any)?.n) {
    const sql = (db.query("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'todos'").get() as { sql: string }).sql;
    db.transaction(() => {
      db.exec(sql.replace(/^CREATE TABLE "?todos"?/i, "CREATE TABLE todos_new").replace(/\bproject_id(\s+)INTEGER\s+NOT\s+NULL/i, "project_id$1INTEGER"));
      db.exec("INSERT INTO todos_new SELECT * FROM todos; DROP TABLE todos; ALTER TABLE todos_new RENAME TO todos;");
    })();
  }
  db.exec(`CREATE TABLE IF NOT EXISTS agents (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, command TEXT NOT NULL, args TEXT NOT NULL DEFAULT '[]', env TEXT NOT NULL DEFAULT '{}', created_at TEXT NOT NULL)`);
  // P4 kept the defaults flat and the mode in projects.mode, both Claude's: they move under "claude".
  for (const p of db.query("SELECT id, mode, defaults FROM projects WHERE mode IS NOT NULL OR defaults IS NOT NULL").all() as { id: number; mode: string | null; defaults: string | null }[]) {
    const d = p.defaults ? JSON.parse(p.defaults) : {};
    const nested = Object.values(d).some((v) => v && typeof v === "object");
    if (nested && !p.mode) continue;
    const all: Defaults = nested ? d : Object.keys(d).length ? { claude: d } : {};
    if (p.mode) all.claude = { ...all.claude, mode: p.mode };
    db.run("UPDATE projects SET defaults = ?, mode = NULL WHERE id = ?", [Object.keys(all).length ? JSON.stringify(all) : null, p.id]);
  }
  // A started TODO without a sessions row was one session, which becomes session 1: every TODO of a database from
  // before sessions, and one started by an older build on this database. Runs on every start, so it catches both.
  db.transaction(() => {
    const ids = (db.query("SELECT id FROM todos t WHERE session_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM sessions s WHERE s.todo_id = t.id)").all() as { id: number }[]).map((r) => r.id);
    for (const id of ids) {
      db.run(`INSERT INTO sessions (todo_id, no, session_id, started_at)
        SELECT id, 1, session_id, COALESCE((SELECT MIN(e.at) FROM events e WHERE e.todo_id = t.id AND e.kind = 'launch'), created_at) FROM todos t WHERE id = ?`, [id]);
      const start = (db.query("SELECT started_at FROM sessions WHERE todo_id = ?").get(id) as { started_at: string }).started_at;
      db.run("UPDATE messages SET session_no = 1 WHERE todo_id = ? AND state = 'delivered' AND session_no IS NULL", [id]);
      db.run("UPDATE events SET session_no = 1 WHERE todo_id = ? AND at >= ? AND session_no IS NULL", [id, start]);
      db.run("UPDATE files SET session_no = 1 WHERE todo_id = ? AND session_no IS NULL", [id]);
    }
  })();
}
