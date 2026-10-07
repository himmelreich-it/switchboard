# Switchboard on ACP: design for the full switch (option B)

Date: 2026-10-07. This follows `acp-investigation.md` and your answers there:
- all three motivations apply (other agents, a live view, permissions in the UI);
- Claude moves to ACP as well;
- the app tools move to an HTTP MCP endpoint.

Everything below targets **stable ACP v1**. The P0 results are in `designs/acp-spike.md`, and every check passed. The open point is pushing a message into a running turn (item 20); see that file. The facts in §2 of the investigation come from reading the adapter's source, not from running it. Phase P0 verifies them before any spec change, which matches the spec's "verified in the spike" convention.

## 1. Architecture

```
Browser UI ──REST/SSE──▶ server.ts (Bun) ──────────────────────────────┐
                           │  runner.ts: delivery, queue, hand-over,    │
                           │  attention rules (unchanged logic)         │
                           │      │                                     │
                           │  AgentDriver (acp.ts)                      │
                           │      │ JSON-RPC over stdio, one child      │
                           │      ▼ per live TODO session               │
                           │  claude-agent-acp │ codex-acp │ gemini --acp│
                           │      │                                     │
                           │      └─ agent calls MCP "app" ─────────────┤
                           │                                            ▼
                           └── POST /mcp/app  (loopback, X-Switchboard-Session secret)
```

- **New `acp.ts`.** It spawns agents, speaks ACP through `@agentclientprotocol/sdk` (`client()` builder, stdio `ndJsonStream`), and exposes the driver API below. It contains no Switchboard logic.
- **New `apptools.ts`.** It holds the worker and operator tool handlers, moved out of the `createSdkMcpServer` closures and keyed by `{todoId}` or `operator`, plus the `/mcp/app` endpoint.
- **`runner.ts`.** It keeps the delivery, queue, session and attention state machine and calls the driver instead of `query()`.
- **Dependencies.** The direct `@anthropic-ai/claude-agent-sdk` dependency goes away; it stays inside the adapter. New dependencies: `@agentclientprotocol/sdk`, `@agentclientprotocol/claude-agent-acp`, `@agentclientprotocol/codex-acp`, and possibly `@modelcontextprotocol/sdk`.

### Process model: one adapter child per live session

One child per live session, rather than one shared child per agent kind, for these reasons:

- **Cancelling stays reliable.** Cancel is `session/cancel`; if the turn has not ended after 10 s, the app kills the child. That is today's `interrupt()` followed by `abort()`. With a shared child the only fallback would be `session/close`.
- **A crash stays contained.** It takes down one TODO, not every running one.
- **Shutdown and startup are simple.** `stopAll` kills every child, and there is nothing to reset on startup.
- **The cost is small.** An extra Node or Bun process sits next to the Claude CLI that each session already spawns. P0 measures the spawn latency.

**Idle children are reaped** after N minutes (default 30). The next delivery spawns a new child and calls `session/resume`. This also bounds memory when many TODOs sit idle.

## 2. AgentDriver

```ts
type AgentKind = string;                        // "claude" | "codex" | "gemini" | custom id from the registry
interface Live {                                 // one child process + one ACP session
  sessionId: string;
  caps: AgentCapabilities;                       // from initialize
  configOptions: ConfigOption[];                 // from session/new|resume and config_option_update
  commands: AvailableCommand[];
  prompt(blocks: ContentBlock[]): Promise<StopReason>;   // resolves at turn end = idle signal
  cancel(): void;                                // session/cancel, kill after 10 s
  setConfig(id: string, value: string | boolean): Promise<ConfigOption[]>;
  close(): void;                                 // session/close + kill
}
interface Handlers {                             // supplied by runner.ts per TODO / operator
  onUpdate(u: SessionUpdate): void;
  onPermission(req: RequestPermissionRequest): Promise<RequestPermissionOutcome>;
  onExit(code: number | null): void;
}
open(kind, {cwd, additionalDirectories, mcp, meta}, h): Promise<Live>        // session/new
reopen(kind, sessionId, {cwd, additionalDirectories, mcp, meta}, h): Promise<Live | "gone">
  // session/resume if caps.sessionCapabilities.resume, else session/load (its replayed updates are dropped), else "gone"
```

What each agent gets in `meta`:

- **Claude:** `_meta.systemPrompt = {append: contract + project prompt}` and `_meta.claudeCode.options = {extraArgs: {"system-prompt-snapshot": "off"}, allowedTools: WORKER_TOOLS}`.
- **Other agents:** nothing. The contract goes in as prompt text instead (§9).

## 3. App tools over HTTP MCP (`/mcp/app`)

- **Endpoint.** `POST /mcp/app` on the existing server. First try `@modelcontextprotocol/sdk`'s web-standard Streamable HTTP server transport in stateless mode. If it does not run under Bun, fall back to a hand-written stateless handler for `initialize`, `notifications/initialized`, `tools/list` and `tools/call`, about 100 lines with zod's `toJSONSchema`.
- **Auth.** Each live session gets a random 32-byte secret, sent in the `X-Switchboard-Session` header through `mcpServers: [{type:"http", name:"app", url, headers}]`.
  - The server keeps an in-memory map from secret to `{todoId}` or `operator`. That restores the "every handler knows its TODO" property from spec 407.
  - Secrets are made fresh on every `open` and `reopen`, and dropped when the child exits.
  - The launch token never reaches an agent.
  - The usual checks still apply: loopback only, the Host check, and the JSON content type. The token middleware lets `/mcp/app` through only with a valid session secret.
- **Fallback for agents without HTTP MCP.** For agents whose `mcpCapabilities.http` is false, use a stdio shim, `bun mcp-stdio.ts`, which forwards to the same endpoint with the secret in an environment variable.
- **Tool names stay `mcp__app__*` for Claude.** Other agents name MCP tools their own way; Codex, for example, uses `app.post_message`. The contract therefore names the tools without the prefix, as "the app's `post_message` tool", and a per-agent string fills in the full name.
- **Per-turn state moves.** `turn.posted` moves from the closure into the live-run record that the handler looks up by `todoId`.

## 4. Session lifecycle

| Today | ACP |
|---|---|
| `openSession` makes a UUID, inserts the `sessions` row, runs `{sessionId}` | Spawn the child, `initialize`, then `session/new`. **Then** insert the `sessions` row with the returned id, set `todos.session_id` and the agent kind, apply config defaults (§7), then `prompt()`. A failure before the insert leaves nothing to roll back, so most of `runner.ts:124-128` goes away. |
| `deliver` uses `query({resume})` | If the TODO has a live child, `prompt()`. Otherwise `reopen()` first. If that returns `"gone"`, post an app message ("Session N can't be resumed: start a new session") and raise attention. |
| Restore uses `getSessionInfo` and the folder checks | The folder checks stay. Then comes a lazy `reopen()` on the first delivery, with the same "gone" handling. |
| `interrupt()` | `cancel()`. The turn ends with `stopReason: cancelled`, which gives the "interrupted" event. Pending permission requests are answered `cancelled`. |
| Startup reset of `running` | The same. No children survive an app restart. |
| Hand-over (`newSession`) | Unchanged, except that the next session may use **a different agent**. The header is plain text, so moving a TODO from Claude to Codex on New session works without extra code. |

Run end works as today. The `prompt()` promise settles, then `statusAfterRun`, the result event, `refreshPrs` and `deliver(queued)` run.

- **Final text.** It is the agent message text accumulated since the last tool call. That keeps today's fallback "agent posted nothing → ≤2-line message".
- **Errors.** A rejected `prompt()` or a child exit becomes the app message and error event, as today.
- **Footer (item 128).** `duration_ms` is measured by the app, and the tool count comes from `tool_call` updates.

## 5. Transcript: stored and streamed

**The app persists the transcript itself.** Not every agent supports `session/load`, and the UI renders from DB rows.

Storage reuses `events` (`kind`, `detail`, `meta`, `session_no`). Rows hold coalesced final items, not chunks:

| ACP update | Stored as |
|---|---|
| `agent_message_chunk` run | One `event kind='say'` with the accumulated text, written when the run of chunks ends (next tool call or turn end) |
| `agent_thought_chunk` run | `kind='thought'`, same rule; collapsed in the UI |
| `tool_call` + `tool_call_update` | One `kind='tool'` row, upserted by `meta.toolCallId`, holding `title`, `kind`, `status`, `locations`, `rawInput`, diff content (`path`, `oldText`, `newText`) and truncated text output. This replaces today's `PreToolUse` rows, and `last_tool` comes from it. |
| `plan` | `kind='plan'`, one row per session, replaced on every update. It also drives a small progress line on the card (`3/7`). |
| `usage_update` | No row. `todos.ctx = {used, size}` drives the context meter (item 123). |
| `current_mode_update`, `config_option_update` | `todos.config` (§7) |
| `available_commands_update` | In memory on the live record, for the composer |

**Live streaming.**
- A new SSE event `{type:"stream", todo, item}` carries the in-progress item, throttled to about 100 ms per TODO.
- The focus view appends and patches it directly. Every other view keeps the "SSE says refetch" model.
- Item rows are persisted when a tool call completes and when the turn ends, so a reload mid-turn shows everything up to the current chunk run.

**UI.**
- The Events switch becomes a **Transcript** view, still a switch in the Chat tab. The curated chat with `post_message` bubbles stays the default.
- With Transcript on, the view shows the agent's text (markdown), collapsed thoughts, and tool rows:
  - an icon by `kind`, the title, and a status dot;
  - expand to see the input and output;
  - diffs render with CodeMirror's merge view, which already ships in `vendor/editor.js`.
- `locations` link to the Files tab, which opens the file at that line.

## 6. Permissions in the UI

- **Mode per project.** Settings › Projects gets "Permission mode", a select filled from the agent's `mode` config option. The default stays `auto` for Claude and the agent's own default for the others. A per-TODO override sits in the focus info line (§7).
- **Requests.** When `request_permission` arrives:
  - Store `{requestId, toolCall, options}` in a new `todos.permission` JSON column. Only one can be open per session, because the agent blocks on it.
  - Set `attention = 1` and emit.
  - Lanes are derived from status and attention: **running + attention shows in Waiting** with a "needs approval" label. Today a raised flag only shows after the run ends.
  - The card and the chat show the tool title, a diff or command preview from `toolCall.content` and `rawInput`, and one button per option (Allow once, Always allow, Reject…).
  - `POST /api/todos/:id/permission {optionId}` resolves the promise, clears the column, and clears attention unless the agent raised it itself (tracked separately as `attention_by`).
- **Cancelled requests.** Cancelling a turn, removing a TODO or shutting down answers an open request with `{outcome:"cancelled"}`.
- **Auto-mode denials** keep today's deterministic path: a `tool_call_update` with `status: failed` and `_meta.claudeCode.toolResponse.decisionReasonType` gives the app message "Auto mode blocked …" plus attention. For other agents a failed tool call is just a failed tool call.
- **The operator never prompts.** It answers every request with reject (§8).

## 7. Config options: model, effort, mode

- **Storage.** `configOptions` from `session/new` and `session/resume` and from `config_option_update` are stored as `todos.config` (JSON).
- **Focus info line.** It renders one compact select per option with category `model`, `thought_level` or `mode`; other categories go behind a "⋯" popover. A change calls `setConfig` on the live session. With no live session, the value is stored and applied after the next `open` or `reopen`.
- **Project defaults.** `projects.defaults` JSON, keyed by agent kind and config id, is set in Settings › Projects and applied right after `session/new`.
- **Slash commands.** `available_commands_update` fills an autocomplete in the composer when the message starts with `/`. Commands go out as plain prompt text, which is how ACP runs them.
- **Stop (item 129, now on main).** It calls `cancel()`.
- **Push into a running turn (item 20, now on main).** It uses the adapter's `_session/steering` extension. See `acp-spike.md` for the error case to tolerate.

## 8. Operator

The operator moves to ACP and stays Claude-only:

- `cwd: ~/.switchboard`
- `mcpServers: [app (operator secret)]`
- `_meta.claudeCode.options = {tools: [Read, Grep, Glob, WebSearch, WebFetch], allowedTools: [those + operator app tools], strictMcpConfig: true, extraArgs: {...}}`
- `_meta.systemPrompt.append = OPERATOR_PROMPT`
- mode `default`
- `onPermission` always selects the `reject_once` option. That emulates `dontAsk`, which ACP does not expose.

The operator's session id stays in `meta.operator_session_id`, and `reopen()` handles restarts.

## 9. Agent registry and multi-agent

- **Agents table (Settings › Agents).** It is seeded with three built-ins:
  - `claude`: the bundled `claude-agent-acp`, spawned with Bun's node compatibility (P0 checks this) or the user's `node`.
  - `codex`: the bundled `codex-acp`. It needs the Codex CLI and the user's login.
  - `gemini`: `gemini --acp` from the login-shell `PATH`.
  - Custom entries add `{id, name, command, args, env}`.
- **Choosing an agent.**
  - `projects.default_agent` sets the default.
  - The New TODO card gets an agent select, set to the project default.
  - New session can switch the agent.
  - `sessions.agent` records the agent per session, `todos.agent` the live one, and cards show a small agent badge.
- **Contract for agents without a system-prompt append.**
  - The contract and project prompt go in as a text block before the first prompt of each session, under a "Switchboard instructions" heading.
  - After an app restart, a `reopen()` of such an agent sends nothing extra, because the agent still has the instructions in its context.
  - **Accepted limitation:** edits to the project prompt reach a live session of another agent only after New session.
- **Authentication.** On `auth_required` from `session/new`:
  - If the agent offers a `terminal` auth method, show an app message with a "Log in" button. It opens a Terminal tab that runs that method's command and args.
  - Otherwise, show the method's description.
- **Agent-specific UI text.**
  - The `claude --resume <id>` hint (item 78) becomes a per-agent template; there is no hint where the agent has none.
  - `contextFile` and the Claude-terminal button stay Claude-only.

## 10. Claude terminals with app tools (the spec line-471 item)

The endpoint now exists, so a Claude terminal can get the app tools too:

- launch with `--mcp-config '{"mcpServers":{"app":{"type":"http","url":…,"headers":{…secret…}}}}'`;
- add `--allowedTools mcp__app__*`;
- add `--append-system-prompt` with the contract.

Such a terminal session gets a `sessions` row with `kind='terminal'`. It can post messages and raise attention, but it is not driven by the app: there is no queue delivery and no transcript. **This phase is optional**, P6.

## 11. Spec changes (items R129 and up)

**Rewritten:**
- run model (spec 407);
- prompt layering (408, now split into Claude `_meta` and the contract as text for other agents);
- denials (409);
- interrupt (410, now cancel with kill after 10 s);
- sessions (412, the id comes from `session/new` and `sessions.agent` is added);
- the context meter (item 123, now `usage_update`);
- the result footer (item 128).

**New:**
- the agent registry and per-session agent;
- the `/mcp/app` endpoint and its security;
- the transcript store and streaming;
- the permission UI and the `todos.permission` column;
- config options and project defaults;
- the Stop button;
- slash-command autocomplete;
- adapter process reaping.

**Struck from "Not built on purpose":**
- "Routing permission prompts to the UI";
- "A model setting";
- the line-471 argument, which is replaced by §10, built or deferred.

**Kept:**
- "Restricting worker agents beyond the user's normal Claude Code config", now generalised to "the agent's own config";
- deterministic attention: permission requests and denials are deterministic signals.

**Spike findings** get a new ACP section from P0.

**Schema changes (additive):**
- `sessions.agent`, `sessions.kind`
- `todos.agent`, `todos.config`, `todos.permission`, `todos.attention_by`
- `projects.default_agent`, `projects.defaults`
- an `agents` table

## 12. Phases

Each phase is one branch and one review, and the app keeps working after every phase.

| Phase | Content | Done when |
|---|---|---|
| **P0 Spike** | A throwaway script in a worktree, using the real adapter and a test HTTP MCP endpoint, that checks the list below. Results go to the spec's spike section. | Every check is recorded as pass or fail with notes |
| **P1 Claude parity** | `acp.ts`, `apptools.ts` and `/mcp/app`; workers and operator on ACP; same UI; denials, the context meter, the result footer and hand-over all as today; Agent SDK dependency removed | Existing tests pass; the start, deliver, queue, new session, restore and remove flows are checked in headless Brave on port 4478 |
| **P2 Live view** | Transcript store, stream SSE, Transcript view, diffs, plan line, Stop button | A running TODO's tool calls and text stream live |
| **P3 Permissions** | Mode per project and per TODO, `request_permission` UI, Waiting-lane rule | A `default`-mode TODO asks in the card, and both Allow and Reject work |
| **P4 Config** | Model and effort selects, project defaults, slash-command autocomplete | Model switch takes effect on the live session |
| **P5 Multi-agent** | Agent registry, Codex and Gemini, agent per TODO and per session, contract as text, auth flow, per-agent hints | A Codex TODO runs end to end with app tools, and hand-over moves it to Claude |
| P6 (optional) | Claude terminals with app tools | n/a |

**P0 checklist:**
1. `claude-agent-acp` runs under `bun`, so Electron packaging works without bundling Node. Otherwise, find out what is needed.
2. `@agentclientprotocol/sdk` `client()` works under Bun.
3. The Claude CLI, through the adapter, connects to a loopback HTTP MCP server with a custom header. Tool calls arrive and are named `mcp__app__*`.
4. `_meta.systemPrompt.append` and `extraArgs` `system-prompt-snapshot: off` reach the CLI. An edited append shows up after `session/resume`.
5. The ACP `sessionId` works with `claude --resume <id>` and shows up under `~/.claude/projects/...`.
6. An auto-mode denial arrives as `tool_call_update` `failed` with `_meta.claudeCode.toolResponse.decisionReasonType`.
7. `session/cancel` during Bash leads to `stopReason: cancelled`, and resuming afterwards works.
8. Killing the child and calling `session/resume` from a new child keeps the context.
9. The shape and frequency of `usage_update`, plus the `configOptions` ids for mode, model and effort.
10. `request_permission` in `default` mode: the options and the toolCall content for Bash and Edit.
11. Spawn-to-first-token latency, against today's `query()`.
12. User settings still load: CLAUDE.md, skills, hooks and user MCP servers.

## 13. Risks

- **Adapter churn.** It is at v0.86 and ships often, with many features under `_meta`. Pin exact versions, and recheck the P0 list on every upgrade (keep the spike script as a smoke test).
- **Bun compatibility.** If the adapter needs real Node, the packaged app has to find or bundle Node. That is a packaging decision for P1.
- **ACP v2 (Draft).** The prompt response stops being the turn end, and `state_update` takes over. All of that sits inside `acp.ts`, behind `prompt(): Promise<StopReason>`.
- **Agent differences.** Contract compliance from other agents (two-line chat, the attention discipline) is untested. P5 may need agent-specific contract wording.
- **Cost of the live view.** Streaming and storing every tool call grows `events`. Truncate tool output (for example 8 KB per row) and keep diffs whole.
