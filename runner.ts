// Agent runs over ACP (acp.ts): each TODO's worker and the operator keep one agent process for their live session, and
// every turn is one session/prompt. The app's tools reach the agent over HTTP MCP (apptools.ts).
import { mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { resolve } from "node:path";
import { openSession as acpOpen, reopenSession, type Command, type Live, type PermissionOutcome, type PermissionRequest, type Update } from "./acp";
import { OPERATOR_TOOLS, WORKER_TOOLS, grant, revoke } from "./apptools";
import {
  attentionAfterAnswer, configChanges, configIn, configView, ctxStep, cutToTwoLines, deliveryAction, deliveryPrompt, handOver, permissionOf, planCount, statusAfterRun, toolArg, toolLine, toolMeta, withAttachments,
  type Ctx, type Permission, type ToolMeta,
} from "./lib";
// server.ts imports this file too (an ES module cycle): use these only inside functions, never at the top level.
import {
  HOME, addEvent, addMessage, artifactDir, attachmentsOf, db, emit, filesOf, getMeta, liveSessionNo, now, projectRow, refreshPrs,
  saveAttachments, sessionsOf, setEvent, setMeta, setTodo, todoRow, type Upload,
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

const OPERATOR_BUILTINS = ["Read", "Grep", "Glob", "WebSearch", "WebFetch"];
// An idle agent process is closed after this; the next turn starts a new one with session/resume.
const IDLE_CLOSE_MS = 30 * 60_000;

type Key = number | "operator"; // a TODO id, or the operator

// What a turn's updates feed: the run's own state, swapped in for every run of a live agent process.
type Sink = { onUpdate(u: Update): void; onPermission(r: PermissionRequest): Promise<any> | any };

// The live agent process of a TODO's (or the operator's) session. `fingerprint` is what session/new or session/resume
// was given (system prompt, folders): when it changes (an edited project prompt), the next turn reopens the session.
type Agent = { live: Live; secret: string; sink: { s: Sink | null }; fingerprint: string; idle?: Timer; configError?: string; skipped?: string; modeMiss?: string };
const agents = new Map<Key, Agent>();

// `live` is set once the agent process is ready; `cancel` records a Stop that came before that; `kill` is the fallback
// timer of an interrupt.
type Run = { done: Promise<void>; live: Live | null; cancel?: boolean; kill?: Timer };
const running = new Map<Key, Run>();
// Runs being interrupted, and by whom: Stop is the user's; remove and shutdown are the app's. Neither gets an error message.
const interrupting = new Map<Key, "user" | "app">();
let shuttingDown = false;

export const isRunning = (key: Key) => running.has(key);
// A worker's open permission request (P3): the agent blocks on it, so there is at most one per TODO. The user's
// answer, a Stop, a removal, a shutdown or the end of the turn settles it.
const pending = new Map<number, { perm: Permission; resolve: (o: PermissionOutcome) => void }>();
// TODOs whose agent posted a chat message in the current turn, so its final text is not posted as well. A set, not run
// state, because the app tools' grant lives as long as the agent process, across runs.
const posted = new Set<number>();

// Like Esc in Claude Code: session/cancel. If the turn has not ended 10 seconds later, the agent process is closed.
export function interrupt(key: Key, by: "user" | "app" = "app") {
  const r = running.get(key);
  if (!r) return;
  if (interrupting.get(key) !== "app") interrupting.set(key, by);
  if (!r.live) {
    r.cancel = true;
    return;
  }
  if (typeof key === "number") cancelPermission(key);
  r.live.cancel();
  clearTimeout(r.kill);
  r.kill = setTimeout(() => running.get(key) === r && r.live?.close(), 10_000);
}

// ---------- permission requests (P3) ----------

// The user's answer to the open request: the agent gets the chosen option, the request leaves the card, and the
// attention flag drops when the request raised it. False when no request is open (answered already, or the turn ended).
export function answerPermission(todoId: number, optionId: string): boolean {
  const p = pending.get(todoId);
  const option = p?.perm.options.find((o) => o.optionId === optionId);
  if (!p || !option) return false;
  pending.delete(todoId);
  const t = todoRow(todoId)!;
  setTodo(todoId, { permission: null, attention: t.attention ? attentionAfterAnswer(t.attention_by) : 0, attention_by: t.attention_by === "permission" ? null : t.attention_by });
  addEvent(todoId, "permission answered", "user", `${option.name}: ${toolLine(p.perm.name, p.perm.input)}`);
  p.resolve({ outcome: "selected", optionId });
  return true;
}

// An open request that can no longer be answered (Stop, remove, shutdown, the turn ended): the agent is told it was cancelled.
function cancelPermission(todoId: number) {
  const p = pending.get(todoId);
  if (!p) return;
  pending.delete(todoId);
  const t = todoRow(todoId)!;
  setTodo(todoId, { permission: null, attention: t.attention ? attentionAfterAnswer(t.attention_by) : 0, attention_by: t.attention_by === "permission" ? null : t.attention_by });
  addEvent(todoId, "permission cancelled", "app", toolLine(p.perm.name, p.perm.input));
  p.resolve({ outcome: "cancelled" });
}

// The agent asks (a mode other than auto, or something auto still asks about): the request goes on the card and in
// the chat, the TODO shows in Waiting, and the turn waits for the user. A request while one is already open (it
// should not happen: the agent blocks) replaces it, and the earlier one is cancelled.
function askPermission(todoId: number, r: PermissionRequest): Promise<PermissionOutcome> {
  cancelPermission(todoId);
  const perm = permissionOf(r, now());
  const t = todoRow(todoId)!;
  if (!t.attention) addEvent(todoId, "attention raised", "app", `${perm.name} needs approval`);
  setTodo(todoId, { permission: JSON.stringify(perm), attention: 1, attention_by: t.attention ? t.attention_by : "permission" });
  addEvent(todoId, "permission asked", "agent", toolLine(perm.name, perm.input));
  return new Promise((resolve) => pending.set(todoId, { perm, resolve }));
}

// The permission mode of a TODO's runs: its own, else the project's, else auto (item 27).
export const modeOf = (t: { mode: string | null }, p: { mode: string | null }) => t.mode || p.mode || "auto";

// Sets the agent's mode when it differs from what the agent reports. A new session starts in the agent's default
// mode, a resumed one may too, a kept process has the mode of its last run (spike finding 9), and a model switch
// resets it (P4 probe), so this runs before every turn, after the other options. When the mode is not available,
// the run goes on in the agent's own mode and the user is told once per miss. Claude takes auto on Haiku without an
// error and stays in acceptEdits (P4 probe), so the mode is checked after the switch too.
async function applyMode(a: Agent, mode: string | null) {
  const cur = () => a.live.configOptions.find((c) => c.id === "mode")?.currentValue;
  if (!mode || cur() === mode) return void (a.modeMiss = "");
  let why = "";
  try {
    await a.live.setConfig("mode", mode);
  } catch (e: any) {
    why = `: ${e?.message ?? e}`;
  }
  if (cur() === mode) return void (a.modeMiss = "");
  if (a.modeMiss === `${mode} ${cur()}`) return;
  a.modeMiss = `${mode} ${cur()}`;
  a.configError = `Could not switch the agent to ${mode} mode; it runs in ${cur() ?? "its own"} mode${why || ", as this model has no " + mode + " mode"}.`;
}

// P4: the TODO's config values (its own over the project's defaults) and then its permission mode, set on the live
// session where they differ from what the agent reports. The model goes first: switching it rewrites the other
// options. A value the agent does not offer (effort on a model without it) is skipped with an event, once per change.
// Runs before every turn and on a change in the Focus header, also mid-turn. `reset` is an option the user just set
// back to the agent's default (no value of the TODO's or the project's): the agent's `default` value is set where it
// offers one (Claude's model and effort), since an unset option would otherwise keep its last value.
export async function applyConfig(todoId: number, a: Agent, reset?: string) {
  const t = todoRow(todoId)!, p = projectRow(t.project_id)!;
  const wanted = { ...configIn(p.defaults ? JSON.parse(p.defaults) : null), ...configIn(t.overrides ? JSON.parse(t.overrides) : null) };
  if (reset && !wanted[reset] && configView(a.live.configOptions).find((o) => o.id === reset)?.options.some((x) => x.value === "default")) wanted[reset] = "default";
  const errors: string[] = [];
  let skipped: [string, string][] = [];
  for (const models of [true, false]) {
    const c = configChanges(configView(a.live.configOptions), wanted, models);
    for (const [id, value] of c.set) {
      try { await a.live.setConfig(id, value); } catch (e: any) { errors.push(`${id} ${value}: ${e?.message ?? e}`); }
    }
    skipped = skipped.concat(c.skipped);
  }
  if (errors.length) a.configError = `Could not set ${errors.join("; ")}`;
  const skip = skipped.map(([id, v]) => `${id} ${v}`).join(", ");
  if (skip && skip !== a.skipped) addEvent(todoId, "config skipped", "app", `the agent does not offer ${skip} now`);
  a.skipped = skip;
  await applyMode(a, modeOf(t, p));
  saveConfig(todoId, a.live);
}

// The session's options as the agent reports them, for the Focus header, and the last list and slash commands seen
// from any session, for Settings and for a TODO without a live agent (meta agent_config, agent_commands).
function saveConfig(todoId: number, live: Live) {
  const config = JSON.stringify(configView(live.configOptions));
  if (config !== "[]") {
    if (todoRow(todoId)?.config !== config) setTodo(todoId, { config }, false);
    if (getMeta("agent_config") !== config) setMeta("agent_config", config);
  }
  if (live.commands.length) setMeta("agent_commands", JSON.stringify(live.commands));
}

// The slash commands the composer completes (P4): the live agent's, else the last ones seen from any session.
export function commandsOf(todoId: number): Command[] {
  const a = liveAgent(todoId);
  if (a?.live.commands.length) return a.live.commands;
  const stored = getMeta("agent_commands");
  return stored ? JSON.parse(stored) : [];
}

// The live agent process of a TODO, when it is still alive (for a mode change between turns).
export const liveAgent = (todoId: number) => { const a = agents.get(todoId); return a && a.live.alive ? a : null; };

// Ends the agent process of a TODO's live session (remove, new session, shutdown). The session stays resumable.
export function closeAgent(key: Key) {
  const a = agents.get(key);
  if (!a) return;
  agents.delete(key);
  clearTimeout(a.idle);
  revoke(a.secret);
  a.live.close();
}

export async function stopAll() {
  shuttingDown = true;
  const runs = [...running.values()];
  for (const key of running.keys()) interrupt(key, "app");
  await Promise.allSettled(runs.map((r) => r.done));
  for (const key of [...agents.keys()]) closeAgent(key);
}

// Marks the messages delivered to the live session and returns their bodies with the paths of their attachments, oldest first.
function take(todoId: number | null, state: "pending" | "queued") {
  const msgs = db.query("SELECT id, body FROM messages WHERE todo_id IS ? AND state = ? ORDER BY id").all(todoId, state) as { id: number; body: string }[];
  const at = now(), no = liveSessionNo(todoId);
  for (const m of msgs) db.run("UPDATE messages SET state = 'delivered', delivered_at = ?, session_no = ? WHERE id = ?", [at, no, m.id]);
  return msgs.map((m) => withAttachments(m.body, attachmentsOf(m.id).map((a) => a.path)));
}

// The agent process for a session: the live one when it still fits, else session/new (`sessionId` null) or
// session/resume in a new process. The config options and the permission mode are set afterwards, by applyConfig().
async function agentFor(key: Key, sessionId: string | null, o: { cwd: string; dirs: string[]; meta: Record<string, unknown>; grant: Parameters<typeof grant>[0] }) {
  const fingerprint = JSON.stringify([o.cwd, o.dirs, o.meta]);
  const had = agents.get(key);
  if (had && had.live.alive && sessionId && had.live.sessionId === sessionId && had.fingerprint === fingerprint) {
    clearTimeout(had.idle);
    return had;
  }
  closeAgent(key);
  const sink: Agent["sink"] = { s: null };
  const handlers = { onUpdate: (u: Update) => sink.s?.onUpdate(u), onPermission: (r: PermissionRequest) => sink.s ? sink.s.onPermission(r) : { outcome: "cancelled" } };
  const g = grant(o.grant);
  const opts = { cwd: o.cwd, additionalDirectories: o.dirs, mcpServers: [g.server], meta: o.meta, env: { SWITCHBOARD_MCP_SECRET: g.secret } };
  let live: Live;
  try {
    live = sessionId ? await reopenSession(sessionId, opts, handlers as any) : await acpOpen(opts, handlers as any);
  } catch (e) {
    revoke(g.secret);
    throw e;
  }
  const a: Agent = { live, secret: g.secret, sink, fingerprint };
  agents.set(key, a);
  return a;
}

// After a run: the process stays for the next turn, and closes when it has been idle for a while.
function parkAgent(key: Key) {
  const a = agents.get(key);
  if (!a) return;
  a.sink.s = null;
  if (!a.live.alive) return closeAgent(key);
  clearTimeout(a.idle);
  a.idle = setTimeout(() => closeAgent(key), IDLE_CLOSE_MS);
}

// One turn's view of the update stream: the final text, the tool calls, the context, and for a worker (`todo` set) the
// transcript (item 130). Rows are written as items complete: the agent's text and thinking when the next tool call or
// the other kind starts, each tool call at once and again on every change. The text being written streams over SSE.
// The text after the last tool call is not written as a row: it is the turn's `result`, as with the SDK.
function turnState(todo: number | null, onDenied: (name: string, input: any, reason: string) => void) {
  const calls = new Map<string, { meta: ToolMeta; eventId: number | null }>();
  const t = { text: "", tools: 0, ctx: null as Ctx | null };
  let buf: { kind: "say" | "thought"; text: string } | null = null;
  let planEvent: number | null = null;
  let streamTimer: Timer | null = null;
  const stream = () => {
    if (todo === null || streamTimer) return;
    streamTimer = setTimeout(() => {
      streamTimer = null;
      emit({ type: "stream", id: todo, item: buf && buf.text.trim() ? { kind: buf.kind, text: buf.text } : null });
    }, 100);
  };
  const flush = (sayToo: boolean) => {
    if (!buf || (buf.kind === "say" && !sayToo)) return;
    if (todo !== null && buf.text.trim()) addEvent(todo, buf.kind, "agent", buf.text.trim());
    if (buf.kind === "say") t.text = "";
    buf = null;
    stream();
  };
  const onUpdate = (u: any) => {
    t.ctx = ctxStep(t.ctx, u);
    const kind = u.sessionUpdate === "agent_message_chunk" ? "say" : u.sessionUpdate === "agent_thought_chunk" ? "thought" : null;
    if (kind && u.content?.type === "text") {
      if (buf && buf.kind !== kind) flush(true);
      buf ??= { kind, text: "" };
      buf.text += u.content.text;
      if (kind === "say") t.text += u.content.text;
      stream();
      return;
    }
    if (u.sessionUpdate === "plan" && todo !== null && Array.isArray(u.entries)) {
      const entries = u.entries.map((e: any) => ({ content: String(e.content ?? ""), status: e.status, priority: e.priority }));
      setTodo(todo, { plan: JSON.stringify(entries) }, false);
      const detail = `${planCount(entries) ?? "0/0"} done`;
      if (planEvent === null) planEvent = addEvent(todo, "plan", "agent", detail, { entries });
      else setEvent(planEvent, todo, detail, { entries });
      return;
    }
    if (u.sessionUpdate !== "tool_call" && u.sessionUpdate !== "tool_call_update") return;
    let c = calls.get(u.toolCallId);
    if (!c) {
      flush(true);
      t.text = "";
      t.tools++;
      const meta = toolMeta(null, u);
      c = { meta, eventId: todo === null ? null : addEvent(todo, "tool", "agent", toolLine(meta.name, meta.input), meta) };
      calls.set(u.toolCallId, c);
      if (todo !== null) setTodo(todo, { last_tool: meta.name }, false);
    } else {
      c.meta = toolMeta(c.meta, u);
      if (c.eventId !== null) setEvent(c.eventId, todo!, toolLine(c.meta.name, c.meta.input), c.meta);
    }
    // A denial (a settings rule or the auto mode classifier) ends the call as failed with the reason in _meta (spike 6).
    const denial = u.status === "failed" && u._meta?.claudeCode?.toolResponse?.decisionReasonType;
    if (denial) onDenied(c.meta.name, c.meta.input, u._meta.claudeCode.toolResponse.decisionReason ?? u._meta.claudeCode.toolResponse.message ?? "");
  };
  // The turn ended: thinking still open is written; the text is the result. The live item is cleared. A tool call
  // that never got a final status (a server tool such as advisor, or one cut off by an interrupt) is marked ended.
  const finish = () => {
    flush(false);
    buf = null;
    for (const c of calls.values()) {
      if (c.meta.status === "completed" || c.meta.status === "failed") continue;
      c.meta = { ...c.meta, status: "ended" };
      if (c.eventId !== null) setEvent(c.eventId, todo!, toolLine(c.meta.name, c.meta.input), c.meta);
    }
    if (todo !== null) emit({ type: "stream", id: todo, item: null });
  };
  return { t, onUpdate, finish };
}

// ---------- workers ----------

// A committed message: stored until start, queued while the agent runs, or sent now (R20). The attachments are saved
// in the same transaction and before deliver(), whose take() reads them. A queued message goes out when the turn ends.
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

// The first turn: every message committed before the start goes out as one prompt.
export function start(todoId: number) {
  runWorker(todoId, () => take(todoId, "pending"), { no: 1 }, "start");
}

// Item 72: ends the live session (it stays as history), clears attention, and starts the next session with the
// hand-over header plus every queued message, because the ended session gets no more runs.
export function newSession(todoId: number) {
  const t = todoRow(todoId)!;
  const prev = sessionsOf(todoId).at(-1)!;
  const no = prev.no + 1;
  db.run("UPDATE sessions SET ended_at = ? WHERE todo_id = ? AND no = ?", [now(), todoId, prev.no]);
  if (t.attention) {
    setTodo(todoId, { attention: 0, attention_by: null, status: t.status === "waiting" ? "idle" : t.status });
    addEvent(todoId, "attention cleared", "user");
  }
  closeAgent(todoId);
  const last = db.query("SELECT body, session_no FROM messages WHERE todo_id = ? AND author = 'agent' ORDER BY id DESC LIMIT 1").get(todoId) as any;
  const header = handOver({
    no, title: t.title, worktree: t.worktree_path, pr: t.pr_number,
    last: last ? { no: last.session_no ?? prev.no, body: last.body } : null,
    files: filesOf(todoId).map((f) => ({ path: f.path, session_no: f.session_no })),
  });
  runWorker(todoId, () => take(todoId, "queued"), { no, header }, "new session");
}

// Resumes the session with every queued message as one prompt. Does nothing while the TODO runs (the run's end calls
// this again), when it is removed or not started, or when nothing is queued.
export function deliver(todoId: number, reason = "deliver") {
  const t = todoRow(todoId);
  if (running.has(todoId) || !t?.session_id || t.removed_at || t.status === "not_started") return;
  const queued = (db.query("SELECT COUNT(*) AS n FROM messages WHERE todo_id = ? AND state = 'queued'").get(todoId) as { n: number }).n;
  if (queued) runWorker(todoId, () => take(todoId, "queued"), null, reason);
}

// `git config worktree.root`, else <local_path>/../worktrees. Created if missing, so the agent always gets a real folder.
export function worktreeRoot(localPath: string) {
  const r = Bun.spawnSync(["git", "-C", localPath, "config", "worktree.root"]);
  const set = r.exitCode === 0 ? r.stdout.toString().trim() : "";
  const root = resolve(localPath, set ? set.replace(/^~(?=\/|$)/, homedir()) : "../worktrees");
  mkdirSync(root, { recursive: true });
  return root;
}

// One run = one turn. `open` starts session `no` (item 68: Start for 1, New session for the rest; `header` is the
// hand-over); null resumes todos.session_id. The TODO shows running at once, while the agent process starts.
// `takeBodies` marks the messages delivered, so it is called only once the agent is ready: a setup error leaves them
// pending or queued, and the user sees an app message instead of messages that look sent but never reached the agent.
function runWorker(todoId: number, takeBodies: () => string[], open: { no: number; header?: string } | null, reason: string) {
  const t = todoRow(todoId)!;
  const p = projectRow(t.project_id)!;
  const dir = artifactDir(p.slug, todoId);
  const run: Run = { done: Promise.resolve(), live: null };
  running.set(todoId, run);
  setTodo(todoId, { status: "running", last_tool: null });
  posted.delete(todoId);

  run.done = (async () => {
    let agent: Agent;
    try {
      mkdirSync(dir, { recursive: true });
      agent = await agentFor(todoId, open ? null : t.session_id, {
        cwd: p.local_path,
        // The worktree root lets a `cd` into the worktree persist; the artifact folder is outside cwd and must be writable.
        dirs: [worktreeRoot(p.local_path), dir],
        meta: {
          systemPrompt: { append: [CONTRACT.replaceAll("{artifact_dir}", dir), p.agent_prompt.trim()].filter(Boolean).join("\n\n") },
          // Without snapshot off, a resumed session keeps the system prompt of its first turn (spike finding 4).
          claudeCode: { options: { allowedTools: WORKER_TOOLS, extraArgs: { "system-prompt-snapshot": "off" } } },
        },
        grant: { kind: "worker", todoId, dir, onPost: () => posted.add(todoId) },
      });
      await applyConfig(todoId, agent);
    } catch (e: any) {
      running.delete(todoId);
      interrupting.delete(todoId);
      if (open && open.no > 1) db.run("UPDATE sessions SET ended_at = NULL WHERE todo_id = ? AND no = ?", [todoId, open.no - 1]);
      addMessage(todoId, "app", cutToTwoLines(`Could not start the run: ${e?.message ?? e}`));
      addEvent(todoId, "error", "app", String(e?.stack ?? e));
      backToStart(todoId, open);
      return;
    }
    const live = agent.live;
    run.live = live;
    // Stopped or removed while the agent process started: a new session is not opened, and nothing is sent.
    if (run.cancel) {
      running.delete(todoId);
      const by = interrupting.get(todoId);
      interrupting.delete(todoId);
      if (open) {
        closeAgent(todoId);
        if (open.no > 1) db.run("UPDATE sessions SET ended_at = NULL WHERE todo_id = ? AND no = ?", [todoId, open.no - 1]);
      } else parkAgent(todoId);
      addEvent(todoId, "interrupted", by ?? "app");
      backToStart(todoId, open);
      if (todoRow(todoId)!.removed_at) closeAgent(todoId);
      // Stopped by the user: the messages stay as they were (item 129). Removed and restored meanwhile: the restore's
      // queued message goes out now, as it would after a run that had started.
      else if (by !== "user" && !shuttingDown) deliver(todoId, "queued messages");
      return;
    }
    if (open) {
      // The agent made the session id. From here on take() and the events belong to the new session.
      db.run("INSERT INTO sessions (todo_id, no, session_id, started_at) VALUES (?, ?, ?, ?)", [todoId, open.no, live.sessionId, now()]);
      setTodo(todoId, { session_id: live.sessionId, ctx: null, plan: null });
      if (open.no > 1) addEvent(todoId, "new session", "user", `session ${open.no} · ${live.sessionId}\n${open.header}`);
    }
    if (agent.configError) {
      addMessage(todoId, "app", cutToTwoLines(agent.configError));
      addEvent(todoId, "error", "app", agent.configError);
      agent.configError = "";
    }
    const cur = todoRow(todoId)!;
    addEvent(todoId, "launch", "app", `${reason} · cwd ${p.local_path} · session ${live.sessionId}`);

    const denied = new Set<string>();
    const deny = (name: string, input: any, why: string) => {
      const what = toolArg(input) || why;
      if (denied.has(name + what)) return;
      denied.add(name + what);
      addMessage(todoId, "app", `Auto mode blocked ${name}: ${what}`);
      const was = todoRow(todoId)!;
      setTodo(todoId, { attention: 1, attention_by: was.attention ? was.attention_by : "app" });
      addEvent(todoId, "attention raised", "app", `${name} denied`);
    };
    const turn = turnState(todoId, deny);
    turn.t.ctx = cur.ctx ? JSON.parse(cur.ctx) : null;
    agent.sink.s = {
      onUpdate: (u) => {
        // The agent changed an option itself (plan mode left, a /model command): the Focus header follows.
        if (u.sessionUpdate === "config_option_update" || u.sessionUpdate === "current_mode_update") saveConfig(todoId, live);
        turn.onUpdate(u);
      },
      // The agent's question goes to the user (P3); the turn waits until it is answered or cancelled.
      onPermission: (r) => askPermission(todoId, r),
    };

    let error = "", stop = "";
    const started = performance.now();
    const prompt = deliveryPrompt(cur.worktree_path, takeBodies(), open?.header ?? "");
    addEvent(todoId, "prompt", "app");
    try {
      stop = await live.prompt(prompt);
    } catch (e: any) {
      error = e?.message ?? String(e);
      if (!live.alive) addEvent(todoId, "agent log", "app", live.stderr().slice(-4000) || null);
      // The Claude adapter rejects a finished turn when a tool call got no result: a server tool such as advisor
      // (ACP spike). When the agent is alive and wrote its final text, the turn is treated as ended; the error stays an event.
      else if (turn.t.text.trim()) {
        addEvent(todoId, "error", "app", `${error} (the turn had finished; treated as ended)`);
        error = "";
      }
    }
    // From here to deliver() nothing awaits, so a message committed meanwhile is either queued and picked up below,
    // or sees the TODO idle and starts its own turn.
    cancelPermission(todoId); // a request still open when the turn ended (the agent died, or gave up waiting)
    turn.finish();
    saveConfig(todoId, live);
    clearTimeout(run.kill);
    running.delete(todoId);
    const stopped = interrupting.get(todoId);
    interrupting.delete(todoId);
    const text = turn.t.text.trim();
    if (turn.t.ctx?.used) setTodo(todoId, { ctx: JSON.stringify({ ...turn.t.ctx, turns: turn.t.ctx.turns + 1 }) }, false);
    if (stop === "refusal" || stop === "max_tokens" || stop === "max_turn_requests") error ||= `The turn ended early: ${stop.replaceAll("_", " ")}.`;
    if (text) addEvent(todoId, "result", "agent", text, { duration_ms: Math.round(performance.now() - started), tools: turn.t.tools });
    if (stopped) addEvent(todoId, "interrupted", stopped);
    if (error && !stopped) {
      addMessage(todoId, "app", cutToTwoLines(error));
      addEvent(todoId, "error", "app", error);
    } else if (!posted.has(todoId) && text) addMessage(todoId, "agent", cutToTwoLines(text));
    const status = statusAfterRun(todoRow(todoId)!.attention);
    setTodo(todoId, { status });
    addEvent(todoId, "run ended", "app", status);
    if (todoRow(todoId)!.removed_at || shuttingDown) closeAgent(todoId);
    else parkAgent(todoId);
    refreshPrs(p.id); // the agent may have pushed or opened a PR
    // Queued messages, including ones committed during the turn, and after a Stop (item 129), go out now.
    if (!shuttingDown) deliver(todoId, "queued messages");
  })();
}

// A run that never sent its prompt: status as before. A first session that did not open goes back to not started, and
// the messages committed while it was starting (queued, because it showed running) become pending, as before a Start.
function backToStart(todoId: number, open: { no: number } | null) {
  if (open?.no === 1) {
    db.run("UPDATE messages SET state = 'pending' WHERE todo_id = ? AND state = 'queued'", [todoId]);
    setTodo(todoId, { status: "not_started" });
  } else setTodo(todoId, { status: statusAfterRun(todoRow(todoId)!.attention) });
}

// ---------- operator ----------

export function commitOperatorMessage(body: string) {
  const id = addMessage(null, "user", body, "queued");
  deliverOperator();
  return id;
}

function deliverOperator() {
  if (running.has("operator")) return;
  if ((db.query("SELECT COUNT(*) AS n FROM messages WHERE todo_id IS NULL AND state = 'queued'").get() as { n: number }).n) runOperator();
}

// The operator keeps its session id in meta.operator_session_id; POST /api/operator/new-session clears it.
function runOperator() {
  const run: Run = { done: Promise.resolve(), live: null };
  running.set("operator", run);
  emit({ type: "operator" });

  run.done = (async () => {
    const stored = getMeta("operator_session_id") ?? null;
    const turn = turnState(null, () => {});
    let agent: Agent;
    try {
      agent = await agentFor("operator", stored, {
        cwd: HOME,
        dirs: [],
        meta: {
          systemPrompt: { append: OPERATOR_PROMPT(HOME) },
          claudeCode: {
            options: {
              tools: OPERATOR_BUILTINS, // no Edit, Write or Bash
              allowedTools: [...OPERATOR_BUILTINS, ...OPERATOR_TOOLS],
              strictMcpConfig: true, // only its own app server, none of the user's MCP servers
              extraArgs: { "system-prompt-snapshot": "off" },
            },
          },
        },
        grant: { kind: "operator" },
      });
    } catch (e: any) {
      // The messages stay queued and are not retried on their own, so a failing start does not loop. A session that
      // cannot be resumed (its transcript cleaned up) is dropped: the next message starts a new one.
      running.delete("operator");
      interrupting.delete("operator");
      if (stored) setMeta("operator_session_id", null);
      addMessage(null, "app", cutToTwoLines(`Could not start the operator: ${e?.message ?? e}${stored ? ". Send again to start a new session." : ""}`));
      emit({ type: "operator" });
      return;
    }
    if (!stored) setMeta("operator_session_id", agent.live.sessionId);
    run.live = agent.live;
    turn.t.ctx = stored && getMeta("operator_ctx") ? JSON.parse(getMeta("operator_ctx")!) : null;
    agent.sink.s = {
      onUpdate: turn.onUpdate,
      // Anything not allowed above is refused instead of asked about, as dontAsk did.
      onPermission: (r) => {
        const no = r.options.find((o) => o.kind === "reject_once") ?? r.options.find((o) => o.kind.startsWith("reject"));
        return no ? { outcome: "selected", optionId: no.optionId } : { outcome: "cancelled" };
      },
    };
    let error = "";
    if (!run.cancel) {
      try {
        await agent.live.prompt(take(null, "queued").join("\n\n"));
      } catch (e: any) {
        error = e?.message ?? String(e);
      }
    }
    clearTimeout(run.kill);
    running.delete("operator");
    if (turn.t.ctx?.used) setMeta("operator_ctx", JSON.stringify({ ...turn.t.ctx, turns: turn.t.ctx.turns + 1 }));
    const stopped = interrupting.delete("operator");
    const text = turn.t.text.trim();
    if (!stopped && error) addMessage(null, "app", cutToTwoLines(error));
    else if (!stopped && text) addMessage(null, "operator", text);
    if (shuttingDown) closeAgent("operator");
    else parkAgent("operator");
    emit({ type: "operator" });
    if (!shuttingDown && !run.cancel) deliverOperator();
  })();
}
