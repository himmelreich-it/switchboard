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

// Why a failed turn is worth continuing: the Mac slept mid-request, or the API failed for a while. null for every other
// error (a session limit, a login, a dead agent), which a retry would not fix.
export function cutReason(error: string): string | null {
  if (error.includes("went to sleep mid-response")) return "the computer slept";
  if (error.includes("Request timed out")) return "the request timed out";
  if (/API Error: 5\d\d\b/.test(error)) return "an API server error";
  return null;
}

export const lineCount =(text: string) => text.trim().split("\n").filter((l) => l.trim()).length;

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

// Change topic: a TODO's worktree, branch and PR per topic it left, keyed by project id ("none": no topic).
export type Place = { worktree_path: string | null; branch: string | null; pr_number: number | null };
// Stores the current place under the topic left and takes the one of the topic entered, if it had one. A stored worktree
// whose folder is gone (`exists` false) comes back without it.
export function swapPlace(places: Record<string, Place>, from: number | null, to: number | null, cur: Place, exists: (p: string) => boolean) {
  const next = { ...places }, key = (id: number | null) => String(id ?? "none");
  if (cur.worktree_path || cur.pr_number) next[key(from)] = cur;
  else delete next[key(from)];
  const b = next[key(to)];
  delete next[key(to)];
  const wt = b?.worktree_path && exists(b.worktree_path) ? b.worktree_path : null;
  return { places: next, back: { worktree_path: wt, pr_number: b?.pr_number ?? null } };
}

// Item 73: what a new session gets from the earlier ones, built from the TODO's data (no agent writes it).
// Lines without a value are left out. `scheduled` is the run line of a scheduled firing (item 144).
export function handOver(o: {
  no: number; title: string; worktree: string | null; pr: number | null;
  last: { no: number; body: string } | null; files: { path: string; session_no: number | null; by_user?: boolean }[]; scheduled?: string | null;
}): string {
  return [
    `This TODO continues earlier work (session ${o.no}). Title: ${o.title}. Keep this title.`,
    o.scheduled && `Scheduled run: ${o.scheduled}.`,
    o.worktree && `Your worktree: ${o.worktree}. Work there.`,
    o.pr && `Pull request: #${o.pr}.`,
    o.last && `Last message of session ${o.last.no}: ${o.last.body.replace(/\s*\n\s*/g, " ")}`,
    o.files.length && "Files from earlier sessions (open with Read what you need):\n"
      + o.files.map((f) => `${f.path} (${f.by_user ? "the user's notes" : f.session_no ? `session ${f.session_no}` : "added outside a session"})`).join("\n"),
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
// task it stopped. How a task ends otherwise (it exits, a plain `kill`) only the SDK's level says, below.
export function backgroundStep(ids: string[], u: any): string[] {
  const cc = u?._meta?.claudeCode, res = cc?.toolResponse;
  if (u?.sessionUpdate === "tool_call_update" && res) {
    if (typeof res.backgroundTaskId === "string") return ids.includes(res.backgroundTaskId) ? ids : [...ids, res.backgroundTaskId];
    const stopped = res.task_id ?? res.shell_id;
    if ((cc.toolName === "TaskStop" || cc.toolName === "KillShell") && ids.includes(stopped)) return ids.filter((id) => id !== stopped);
  }
  return ids;
}

// The SDK's background_tasks_changed, forwarded raw by the adapter when the session asks for it (spike 2026-10-11):
// the full set of live background tasks on every change, so it replaces the list. A task killed by a plain `kill`
// leaves it at once, with no follow-up cycle. Only shell tasks count, as above; `ambient` ones are not activity.
export function backgroundLevel(ids: string[], m: any): string[] {
  if (m?.type !== "system" || m.subtype !== "background_tasks_changed" || !Array.isArray(m.tasks)) return ids;
  const live = m.tasks.filter((t: any) => t.task_type === "local_bash" && !t.ambient).map((t: any) => String(t.task_id));
  const next = [...ids.filter((id) => live.includes(id)), ...live.filter((id: string) => !ids.includes(id))];
  return next.length === ids.length && next.every((id, i) => id === ids[i]) ? ids : next;
}

// The names the SDK's level gives the tasks (their Bash `description`), kept by id; only the shell tasks of
// backgroundLevel, so a subagent starting does not refetch the board. A task that joined by its Bash result before the
// level arrived has no name until then.
export function backgroundNames(names: Record<string, string>, m: any): Record<string, string> {
  if (m?.type !== "system" || m.subtype !== "background_tasks_changed" || !Array.isArray(m.tasks)) return names;
  const add = m.tasks.filter((t: any) => t.task_type === "local_bash" && !t.ambient && t.description && names[t.task_id] !== t.description);
  return add.length ? { ...names, ...Object.fromEntries(add.map((t: any) => [String(t.task_id), String(t.description)])) } : names;
}

// ---------- ports (the Running view's Scan) ----------

// `lsof -nP -iTCP -sTCP:LISTEN -Fpcn`: a `p<pid>` line, a `c<name>` line, then an `f`/`n<addr:port>` pair per socket.
// One entry per process, its ports ascending (IPv4 and IPv6 of one port count once).
export function parseListen(out: string): { pid: number; name: string; ports: number[] }[] {
  const procs = new Map<number, { pid: number; name: string; ports: number[] }>();
  let cur: { pid: number; name: string; ports: number[] } | null = null;
  for (const line of out.split("\n")) {
    const v = line.slice(1);
    if (line[0] === "p") { const pid = Number(v); cur = procs.get(pid) ?? { pid, name: "", ports: [] }; procs.set(pid, cur); }
    else if (line[0] === "c" && cur) cur.name = v;
    else if (line[0] === "n" && cur) {
      const port = Number(v.slice(v.lastIndexOf(":") + 1));
      if (port && !cur.ports.includes(port)) cur.ports.push(port);
    }
  }
  return [...procs.values()].filter((p) => p.ports.length).map((p) => ({ ...p, ports: p.ports.sort((a, b) => a - b) }));
}

// `lsof -a -p <pids> -d cwd -Fn`: `p<pid>`, then `fcwd`, then `n<folder>`.
export function parseCwd(out: string): Map<number, string> {
  const dirs = new Map<number, string>();
  let pid = 0;
  for (const line of out.split("\n")) {
    if (line[0] === "p") pid = Number(line.slice(1));
    else if (line[0] === "n" && pid) dirs.set(pid, line.slice(1));
  }
  return dirs;
}

// `ps -A -o pid=,etime=,command=`: seconds since start (etime is "[[dd-]hh:]mm:ss") and the full command per pid.
export function parsePs(out: string): Map<number, { secs: number; command: string }> {
  const rows = new Map<number, { secs: number; command: string }>();
  for (const line of out.split("\n")) {
    const m = line.trim().match(/^(\d+)\s+(?:(\d+)-)?(?:(\d+):)?(\d+):(\d+)\s+(.*)$/);
    if (m) rows.set(Number(m[1]), { secs: ((Number(m[2] ?? 0) * 24 + Number(m[3] ?? 0)) * 60 + Number(m[4])) * 60 + Number(m[5]), command: m[6]! });
  }
  return rows;
}

// `docker ps --format` with DOCKER_FORMAT: one container per line. Only published ports count: "0.0.0.0:8000->8000/tcp,
// [::]:8000->8000/tcp" is 8000; an unpublished "8443/tcp" is not on the host.
export const DOCKER_FORMAT = '{{.Names}}|{{.Ports}}|{{.Label "com.docker.compose.project.working_dir"}}|{{.RunningFor}}|{{.Image}}';
export function parseDocker(out: string): { name: string; ports: number[]; folder: string; age: string; image: string }[] {
  return out.split("\n").filter((l) => l.includes("|")).map((l) => {
    const [name = "", ports = "", folder = "", age = "", image = ""] = l.split("|");
    const host = [...ports.matchAll(/:(\d+)->/g)].map((m) => Number(m[1]));
    return { name, ports: [...new Set(host)].sort((a, b) => a - b), folder, age, image };
  });
}

// Whose folder a process or container runs in: the TODO whose worktree holds it, else the topic whose folder does.
// The deepest folder wins, so a worktree inside the topic folder beats the topic.
// ponytail: by folder only. A process started from `/` or that changed folder matches nothing; the process tree
// below an agent process would catch it while that agent lives.
export function ownerOf(folder: string, todos: { id: number; worktree_path: string | null }[], projects: { id: number; local_path: string | null }[]) {
  if (!folder) return { todo_id: null, project_id: null };
  const within = (dir: string | null) => !!dir && (folder === dir || isInside(folder, dir));
  const t = todos.filter((x) => within(x.worktree_path)).sort((a, b) => b.worktree_path!.length - a.worktree_path!.length)[0];
  const p = projects.filter((x) => within(x.local_path)).sort((a, b) => b.local_path!.length - a.local_path!.length)[0];
  return { todo_id: t?.id ?? null, project_id: p?.id ?? null };
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

// The board lane: a running TODO with an open permission request needs the user now, so it shows in Waiting. So does
// one that runs only for background tasks (item 136) after the agent raised attention itself: the agent is done and
// asks, a Monitor of its own still runs. Otherwise the lane is the status. The tab title and ⌘J count lanes, not statuses.
// A scheduled TODO that is idle or not started is in the Scheduled lane (item 144); running or waiting, it is where any TODO is.
export const laneOf = (t: { status: Status; permission: Permission | null; background_wait?: boolean; attention_by?: string | null; schedule?: unknown }): Status | "scheduled" =>
  t.status === "running" && (t.permission || (t.background_wait && t.attention_by === "agent")) ? "waiting"
    : t.schedule && (t.status === "idle" || t.status === "not_started") ? "scheduled" : t.status;

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

// Auto effort (Claude only): the levels the router and the worker may pick, low to high. max stays a user choice.
export const AUTO_EFFORTS = ["low", "medium", "high", "xhigh"] as const;
// The effort option's id and the value auto sets: the picked level (null: medium), else the next lower one the agent
// offers. Null when the wanted values set effort already (the TODO's or its topic's) or the agent has no effort option.
export function autoEffort(current: ConfigOption[], wanted: Record<string, string>, level: string | null): [string, string] | null {
  const o = current.find((c) => c.category === "thought_level");
  if (!o || wanted[o.id]) return null;
  const i = AUTO_EFFORTS.indexOf(level as any), top = i < 0 ? AUTO_EFFORTS.indexOf("medium") : i;
  const v = AUTO_EFFORTS.slice(0, top + 1).reverse().find((x) => o.options.some((y) => y.value === x));
  return v ? [o.id, v] : null;
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

// The gh accounts logged in to github.com. `gh auth status --json hosts` is missing from older gh versions
// (Ubuntu ships 2.46) reject the flag, so their text output is read instead: "Logged in to github.com account X" (2.40+)
// or "Logged in to github.com as X" (older).
export function parseGhLogins(json: string | null, text: string): string[] {
  try { if (json) return (JSON.parse(json).hosts?.["github.com"] ?? []).map((h: any) => h.login); } catch {}
  return [...new Set([...text.matchAll(/Logged in to github\.com (?:account|as) (\S+)/g)].map((m) => m[1]!))];
}

const PR_FIELDS = `fragment F on PullRequest { number url state isDraft mergeable mergeStateStatus reviewDecision baseRefName
  reviewThreads(first: 100) { nodes { isResolved } }
  commits(last: 1) { nodes { commit { statusCheckRollup { state contexts(first: 100) { nodes {
    __typename ... on CheckRun { name status conclusion } ... on StatusContext { context state } } } } } } } }`;

// One GraphQL query for a whole repository: alias b<i> finds the newest PR whose head is branch $b<i>,
// alias n<i> reads PR number $n<i> (registered with set_pr). Values go in as variables, never into the query text.
export function prQuery(branches: number, numbers: number, fields = PR_FIELDS) {
  const vars = ["$o: String!", "$n: String!"], parts: string[] = [];
  for (let i = 0; i < branches; i++) {
    vars.push(`$b${i}: String!`);
    parts.push(`b${i}: pullRequests(headRefName: $b${i}, first: 1, orderBy: {field: CREATED_AT, direction: DESC}) { nodes { ...F } }`);
  }
  for (let i = 0; i < numbers; i++) {
    vars.push(`$n${i}: Int!`);
    parts.push(`n${i}: pullRequest(number: $n${i}) { ...F }`);
  }
  return `query(${vars.join(", ")}) { repository(owner: $o, name: $n) { ${parts.join(" ")} } }\n${fields}`;
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

// ---------- worktree scan (Settings › Worktrees) ----------

// The scan needs a PR's state and its commits: a squash merge or a deleted branch leaves no other trace of them.
export const SCAN_PR_FIELDS = "fragment F on PullRequest { number url state commits(first: 250) { nodes { commit { oid } } } }";

// `git worktree list --porcelain`: blocks split by a blank line, the first one the main checkout.
//   worktree /Users/me/code/worktrees/fix-x
//   HEAD 3f2a…
//   branch refs/heads/fix-x        (or "detached")
//   prunable gitdir file points to non-existent location
export function parseWorktreeList(out: string) {
  return out.split(/\n\s*\n/).filter((b) => b.trim()).map((block) => {
    const w = { path: "", head: "", branch: null as string | null, prunable: false, locked: false };
    for (const line of block.split("\n")) {
      const i = line.indexOf(" "), k = i < 0 ? line : line.slice(0, i), v = i < 0 ? "" : line.slice(i + 1);
      if (k === "worktree") w.path = v;
      else if (k === "HEAD") w.head = v;
      else if (k === "branch") w.branch = v.replace(/^refs\/heads\//, "");
      else if (k === "prunable") w.prunable = true;
      else if (k === "locked") w.locked = true;
    }
    return w;
  });
}

export type WorktreeFacts = {
  prunable: boolean; locked: boolean; branch: string | null;
  changed: string[];      // `git status --porcelain` lines: uncommitted and untracked files
  localOnly: string[];    // "<sha> <subject>" of commits on no remote branch and not in the PR
  pr: { number: number; url: string; state: "open" | "merged" | "closed" } | null;
  todos: { id: number; removed: boolean }[];
  branchOnly?: boolean;   // a local branch no worktree has checked out: no TODO can work in it
};
export type Verdict = { verdict: "remove" | "check" | "keep"; reasons: string[] };

// Keep: a live TODO works in it, or its PR is open. Remove: its PR is merged or closed and nothing in it exists only
// here (no changed file, no commit on no remote). Check: anything else. Keep rows still list what exists only here.
// Also for a branch without a worktree (branchOnly): then only its PR and its commits count.
export function worktreeVerdict(f: WorktreeFacts): Verdict {
  if (f.prunable) return { verdict: "remove", reasons: ["folder is gone; git still lists it"] };
  const n = (k: number, one: string, many: string) => `${k} ${k === 1 ? one : many}`;
  const live = f.todos.filter((t) => !t.removed).map((t) => `T-${t.id}`);
  const risks = [
    f.changed.length && n(f.changed.length, "changed file", "changed files"),
    f.localOnly.length && n(f.localOnly.length, "commit on no remote", "commits on no remote"),
    f.locked && "locked",
  ].filter((r): r is string => !!r);
  const pr = f.pr ? `PR #${f.pr.number} ${f.pr.state}` : f.branch ? "no PR" : "detached HEAD";
  const todo = live.length ? `${live.join(", ")} live` : f.todos.length ? `${f.todos.map((t) => `T-${t.id}`).join(", ")} removed` : "no TODO";
  const reasons = [pr, ...(f.branchOnly ? [] : [todo]), ...risks];
  if (live.length || f.pr?.state === "open") return { verdict: "keep", reasons };
  return { verdict: f.pr && !risks.length ? "remove" : "check", reasons };
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

// The command that opens a folder in VS Code. macOS finds the app by its bundle id, so the `code` shell command need
// not be installed. Linux runs `code` from PATH. On Windows `code` is code.cmd, which only cmd.exe runs; /s makes cmd
// drop the outer quotes, and the caller spawns it with windowsVerbatimArguments so Bun adds none of its own.
// ponytail: a % in a Windows path would expand as a variable; such folder names are rare.
export function vscodeCommand(platform: string, dir: string): string[] {
  if (platform === "darwin") return ["open", "-b", "com.microsoft.VSCode", dir];
  if (platform === "win32") return ["cmd.exe", "/d", "/s", "/c", `"code "${dir.replaceAll("/", "\\")}""`];
  return ["code", dir];
}

// The processes below `root` in `ps -A -o pid=,ppid=` output (lines "pid ppid"), children before grandchildren.
export function descendants(ps: string, root: number): number[] {
  const kids = new Map<number, number[]>();
  for (const line of ps.split("\n")) {
    const [pid, ppid] = line.trim().split(/\s+/).map(Number);
    if (pid && ppid !== undefined && !Number.isNaN(ppid)) kids.set(ppid, [...(kids.get(ppid) ?? []), pid]);
  }
  const out: number[] = [];
  for (let i = -1; i < out.length; i++) for (const k of kids.get(i < 0 ? root : out[i]!) ?? []) if (k !== root && !out.includes(k)) out.push(k);
  return out;
}

// True when `version` ("1.3.14") is at least `min` ("1.3.5").
export function versionAtLeast(version: string, min: string): boolean {
  const a = version.split(".").map(Number), b = min.split(".").map(Number);
  for (let i = 0; i < 3; i++) if ((a[i] ?? 0) !== (b[i] ?? 0)) return (a[i] ?? 0) > (b[i] ?? 0);
  return true;
}

// What the worker is told about where it runs, between the contract and the topic prompt. The user files TODOs under
// topics; the agent needs a folder, so the intro names both, and where worktrees go (`worktrees`: null outside git).
export function topicIntro(p: { name: string; local_path: string; independent?: number | boolean | null } | null, worktrees: string | null) {
  if (!p) return "This TODO has no topic: it is thinking only, with no code changes. Your working directory is its artifact folder. Do not create a worktree.";
  const head = `This TODO belongs to the topic "${p.name}". The user knows it by that name; its folder on disk, ${p.local_path}, is your working directory.`;
  const intro = worktrees
    ? `${head} The folder is a git repository. Before you start work, pull the latest main: \`git pull --ff-only\` when the folder is on main, else \`git fetch origin main:main\`. Worktrees go under ${worktrees}, e.g. \`git worktree add -b <branch> ${worktrees}/<name> main\`; the topic prompt may say more.`
    : `${head} The folder is not a git repository: work in it directly, without a worktree, branches or a pull request, and do not run git init.`;
  return p.independent ? `${intro}\n\n${independentText(!!worktrees)}` : intro;
}

// Independent mode: the user's standing go-ahead to ship work in this topic without them. It overrides the contract's
// "outward actions need an explicit message" for push, pull request and merge only; Jira, Slack and the rest still ask.
const independentText = (git: boolean) => [
  `Independent mode is on for this topic. The user turned it on so work here ships without them. It is their standing go-ahead for the steps below, also in a TODO another TODO's agent created, and it is the confirmation that the contract above and your own instructions ask for before these steps.`,
  ...(git ? [`- Ship your change yourself once you trust it: the tests pass, you read the whole diff, and it does what the TODO asks. Commit, push the branch and open a pull request. When the repository has checks, wait for them (\`gh pr checks --watch\`; \"no checks reported\" means there are none) and fix a failing one, or ask. Then squash-merge it with \`gh pr merge <number> --squash\` and pull the latest main. A merge the repository refuses (branch protection, review required): ask.`] : []),
  `- On a design question with a recommended option you are confident in, pick it and go on. Post one line naming the choice and why. Not confident: ask, as usual.`,
  `- Still ask before anything destructive or hard to undo (force-push, history rewrite, deleting data) and before anything outside this topic's repository (Jira, Slack, other repositories).`,
].join("\n");

// Item 114: what a Claude terminal reads first, written to <artifact folder>/terminal/context.md at each launch.
export function contextFile(c: {
  title: string; project: string; status: string; first: string; worktree: string | null; branch: string | null;
  git: string | null; pr: string | null; sessions: { no: number; session_id: string; started_at: string; ended_at: string | null }[];
  messages: { author: string; body: string; at: string; session_no: number | null }[]; files: string[];
}): string {
  const out: string[] = [
    `# Switchboard TODO: ${c.title}`, "",
    `- Topic: ${c.project}`, `- Status: ${c.status === "not_started" ? "not started (in the backlog)" : c.status}`,
    ...[c.worktree && `- Worktree: ${c.worktree}`, c.branch && `- Branch: ${c.branch}`, c.git && `- Git: ${c.git}`, c.pr && `- Pull request: ${c.pr}`].filter((l): l is string => !!l),
    "", "## First instruction", "", c.first, "",
    "## Note", "",
    "The TODO's own agent may be working in the same folder or worktree at the same time. Check with the user before you commit, switch branches or rewrite history.", "",
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

// ---------- schedules (item 144) ----------

// Days 0 = Sunday … 6 = Saturday; times "HH:MM" in the machine's local time. Either fixed `times`, or `every` N minutes
// from `from` to `to` (both included; without them the whole day). `prompt` null: the first instruction is the prompt.
export type Schedule = { days: number[]; times?: string[]; every?: number; from?: string; to?: string; prompt: string | null; paused: boolean };
const HM = /^([01]\d|2[0-3]):[0-5]\d$/;
const toMin = (hm: string) => Number(hm.slice(0, 2)) * 60 + Number(hm.slice(3));
const p2 = (n: number) => String(n).padStart(2, "0");
const DAY = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MON = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

// A schedule from a request body or a tool call, or the error the user or agent reads.
export function scheduleIn(v: any): Schedule | string {
  if (!v || typeof v !== "object" || Array.isArray(v)) return "schedule must be an object";
  const days = ([...new Set(Array.isArray(v.days) ? v.days : [])] as any[]).filter((d) => Number.isInteger(d) && d >= 0 && d <= 6).sort() as number[];
  if (!days.length) return "Pick at least one day (days: 0 = Sunday … 6 = Saturday).";
  const prompt = typeof v.prompt === "string" && v.prompt.trim() ? v.prompt.trim() : null;
  const base = { days, prompt, paused: v.paused === true };
  if (v.every != null) {
    const every = Number(v.every);
    if (!Number.isInteger(every) || every < 1 || every > 1440) return "every is minutes between runs, 1 to 1440.";
    const from = v.from || undefined, to = v.to || undefined;
    if ([from, to].some((x) => x !== undefined && !HM.test(x))) return "from and to are times like 08:00.";
    if (from && to && toMin(from) > toMin(to)) return "from must come before to.";
    return { ...base, every, ...(from ? { from } : {}), ...(to ? { to } : {}) };
  }
  const times = [...new Set(Array.isArray(v.times) ? v.times : [])] as string[];
  if (!times.length || !times.every((x) => typeof x === "string" && HM.test(x))) return "Give times like 09:00, or every N minutes.";
  return { ...base, times: times.sort() };
}

// A day's slots in minutes after midnight.
const slotsOf = (s: Schedule) => {
  if (!s.every) return (s.times ?? []).map(toMin);
  const out: number[] = [];
  for (let m = s.from ? toMin(s.from) : 0; m <= (s.to ? toMin(s.to) : 1439); m += s.every) out.push(m);
  return out;
};

// The first slot later than `after`, or null. Built from date parts, not by adding 24 hours, so a slot stays at its
// wall-clock time across a daylight saving change. A slot in the skipped spring-forward hour runs an hour later.
export function nextRun(s: Schedule, after: Date): Date | null {
  const slots = slotsOf(s);
  for (let d = 0; d < 8; d++) {
    const day = new Date(after.getFullYear(), after.getMonth(), after.getDate() + d);
    if (!s.days.includes(day.getDay())) continue;
    for (const m of slots) {
      const at = new Date(day.getFullYear(), day.getMonth(), day.getDate(), Math.floor(m / 60), m % 60);
      if (at > after) return at;
    }
  }
  return null;
}

// "Weekdays 09:00", "Mon–Fri 09:00, 14:00", "Daily 09:00", "Mon 08:00", "Every 30 min · 08–18", "Every 2 h": the chip
// text (design v9) and the hand-over's run line.
export function scheduleText(s: Schedule): string {
  if (s.every) return `Every ${s.every % 60 ? `${s.every} min` : `${s.every / 60} h`}${s.from && s.to ? ` · ${s.from.slice(0, 2)}–${s.to.slice(0, 2)}` : ""}`;
  const times = s.times ?? [], weekdays = s.days.join() === "1,2,3,4,5";
  const days = s.days.length === 7 ? "Daily" : weekdays ? (times.length > 1 ? "Mon–Fri" : "Weekdays") : [1, 2, 3, 4, 5, 6, 0].filter((d) => s.days.includes(d)).map((d) => DAY[d]).join(", ");
  return `${days} ${times.join(", ")}`;
}

// "Tue 13 Oct 09:00", for events and the hand-over.
export const slotText = (d: Date) => `${DAY[d.getDay()]} ${d.getDate()} ${MON[d.getMonth()]} ${p2(d.getHours())}:${p2(d.getMinutes())}`;

// Why a firing does not run (design §3.3), or null when it runs. A routing run or one asking counts as running or waiting.
export function skipReason(t: { status: Status; attention: number | boolean; permission: string | null; routing: string | null }, busy: boolean, queued: number): string | null {
  if (t.permission) return "waiting for approval";
  if (t.attention || t.status === "waiting") return "waiting on you";
  if (busy || t.status === "running" || t.routing) return "still running";
  if (queued) return "messages waiting";
  return null;
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
CREATE TABLE IF NOT EXISTS comments (
  id INTEGER PRIMARY KEY, todo_id INTEGER NOT NULL, root TEXT NOT NULL, rel TEXT NOT NULL, path TEXT NOT NULL,
  quote TEXT NOT NULL, line INTEGER, body TEXT NOT NULL, reply TEXT,
  state TEXT NOT NULL CHECK (state IN ('open','replied','resolved')), created_at TEXT NOT NULL, replied_at TEXT);
CREATE INDEX IF NOT EXISTS messages_todo ON messages (todo_id, state);
CREATE INDEX IF NOT EXISTS comments_todo ON comments (todo_id);
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
  addColumn("files", "by_user", "INTEGER");      // 1: a note the user made with New note; editable while the agent runs
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
  addColumn("todos", "places", "TEXT");          // worktree, branch and PR per topic the TODO left (JSON {project id: Place}), Change topic
  // When the TODO's topic (or "no topic") was decided. A TODO made before routing got its project from the user.
  if (addColumn("todos", "routed_at", "TEXT")) db.run("UPDATE todos SET routed_at = created_at");
  addColumn("projects", "profile", "TEXT");      // what lives in the repo, written by a scan or the user, for routing
  addColumn("projects", "profile_at", "TEXT");   // when the last scan wrote the profile
  addColumn("projects", "profile_edited", "INTEGER"); // 1 once the user edited the profile; a Rescan asks first
  addColumn("projects", "independent", "INTEGER");   // 1: Independent mode, the agent may push, open and squash-merge its PR
  addColumn("todos", "schedule", "TEXT");        // the repeat rule (JSON Schedule), item 144; null: not scheduled
  addColumn("todos", "next_run_at", "TEXT");     // the next slot; null when paused or not scheduled
  addColumn("todos", "parent_id", "INTEGER");    // the TODO whose agent created this one (item 145)
  addColumn("todos", "key", "TEXT");             // create_todo's dedupe key, unique below
  addColumn("todos", "auto_effort", "TEXT");     // the effort the router or the agent picked, {value, reason} (JSON); null: medium
  // A TODO has no repo before routing, or none at all: project_id becomes nullable. SQLite cannot drop NOT NULL, so the
  // table is rebuilt from its own CREATE statement. It has no triggers; its one index (todos_key) is made after this.
  if ((db.query("SELECT \"notnull\" AS n FROM pragma_table_info('todos') WHERE name = 'project_id'").get() as any)?.n) {
    const sql = (db.query("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'todos'").get() as { sql: string }).sql;
    db.transaction(() => {
      db.exec(sql.replace(/^CREATE TABLE "?todos"?/i, "CREATE TABLE todos_new").replace(/\bproject_id(\s+)INTEGER\s+NOT\s+NULL/i, "project_id$1INTEGER"));
      db.exec("INSERT INTO todos_new SELECT * FROM todos; DROP TABLE todos; ALTER TABLE todos_new RENAME TO todos;");
    })();
  }
  // Item 145: a removed TODO keeps its key, so it blocks re-creation too.
  db.exec("CREATE UNIQUE INDEX IF NOT EXISTS todos_key ON todos (key) WHERE key IS NOT NULL");
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
