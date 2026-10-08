# Agent Client Protocol (ACP) for Switchboard: investigation and design

Date: 2026-10-07. Sources: the ACP spec repo (agentclientprotocol/agent-client-protocol, schema 1.10.2), the Claude adapter source (`@agentclientprotocol/claude-agent-acp` v0.86.0, read at HEAD 0724b17), and this repo at 189402c.

## TL;DR

- **ACP does not replace MCP. It replaces the way the app drives the agent.** Stable ACP gives a client no channel of its own for tools. A client that wants the agent to call its tools must pass an MCP server in `session/new`; the spec says so. ACP's own tool channel, "MCP-over-ACP", is a Draft RFD, and the Claude adapter does not implement it. It drops `type:"acp"` servers without an error.
- So under ACP the six `mcp__app__*` tools stay MCP tools. The difference is that they must run **outside the process**, as a stdio or HTTP MCP server. Today they are an in-process `createSdkMcpServer`. The spec explicitly avoids an out-of-process server (spec lines 407, 471).
- What ACP does replace is `runner.ts`'s driving layer: `query()`/`resume`, `drain()`, the in-process hooks, `interrupt()` and the usage tracking. In their place come `session/new|load|resume`, `session/prompt`, the `session/update` stream and `session/cancel`.
- What ACP *buys* falls into two groups:
  1. Things on the spec's "Not built on purpose" list: a live transcript stream (tool calls, diffs, plan, thinking), permission prompts in the UI, and model and effort selection.
  2. **Other agents** behind one interface: Codex, Gemini CLI, Goose, Copilot CLI, OpenCode and more.

  Whether to adopt ACP comes down to whether you want those things, not to the protocol itself.
- **Recommendation:** first introduce an ACP-shaped `AgentDriver` seam, with the current SDK code as its only implementation (option C). Then add an ACP driver behind it, plus an HTTP MCP endpoint for the app tools, when multi-agent support or a live transcript becomes a goal. Details are in the last section.

## 1. What the protocol offers (stable v1 unless marked otherwise)

| Area | ACP v1 | Status |
|---|---|---|
| Transport | JSON-RPC 2.0, newline-delimited over stdio; the client spawns the agent | Stable |
| Remote transport | Streamable HTTP + WebSocket | Active RFD, not stable |
| Handshake | `initialize` → `agentCapabilities` (`loadSession`, `promptCapabilities`, `mcpCapabilities{http,sse}`, `sessionCapabilities{list,resume,close,delete,additionalDirectories}`), `authMethods` | Stable |
| Sessions | `session/new {cwd, mcpServers, additionalDirectories}` → agent-assigned `sessionId`; `session/load` (replays history as updates); `session/resume` (no replay); `session/list`, `session/close`, `session/delete` | Stable |
| Fork | `session/fork` | Draft |
| Turn | `session/prompt {prompt: ContentBlock[]}` → `{stopReason: end_turn\|max_tokens\|max_turn_requests\|refusal\|cancelled}`. **The response is the end of the turn.** | Stable |
| Cancel | `session/cancel`; the agent ends with `stopReason: cancelled` | Stable |
| Stream | `session/update`: `agent_message_chunk`, `agent_thought_chunk`, `user_message_chunk`, `tool_call` / `tool_call_update` (kind, status, content: text, `diff`, `terminal`; locations; rawInput/rawOutput), `plan`, `available_commands_update`, `current_mode_update`, `config_option_update`, `session_info_update`, `usage_update {used, size, cost}` | Stable |
| More updates | `compaction_update`, `notice` (Preview); `subagent_update`, `plan_update` (Draft) | Unstable |
| Permissions | `session/request_permission {toolCall, options[allow_once\|allow_always\|reject_once\|reject_always]}` → selected option or `cancelled` | Stable |
| Settings | `session/set_config_option` with categories `mode`, `model`, `model_config`, `thought_level`. `session/set_mode` is deprecated, and `set_model` has been removed. | Stable |
| Slash commands | Advertised in `available_commands_update` and sent as plain prompt text | Stable |
| Client services | `fs/read_text_file`, `fs/write_text_file`, `terminal/*` (removed in v2), `elicitation/create` (form and URL) | Stable in v1 |
| Extensions | `_meta` on most types; `_`-prefixed custom methods and notifications | Stable |
| Client-hosted tools | **Only by passing MCP servers.** MCP-over-ACP (`type:"acp"` server, `mcp/message`) is Draft. | Draft |
| v2 | `state_update` (running / idle / requires_action) separates turn end from the prompt response; `messageId` becomes required; `fs` and `terminal` are dropped | Draft (since 2026-07-20) |

## 2. What the Claude adapter actually does (checked in source)

These four questions decide whether the app can move to ACP. All four come out fine.

1. **The user's full Claude Code config is loaded.** `settingSources: ["user","project","local"]` is the default (`src/acp-agent.ts:9471`). It can be overridden through `_meta.claudeCode.options`, which passes most SDK `Options` through (`NewSessionMeta`, about line 1480). That gives CLAUDE.md, skills, hooks and the user's MCP servers, which the spec promises on lines 5 and 53.
2. **The session id is the Claude session id.** On `session/new` the adapter creates a UUID and passes it to the SDK as `options.sessionId`. `session/load` and `session/resume` pass the same id as `resume` (lines 9240–9256 and 9626). So `claude --resume <id>`, the `~/.claude/projects/...` transcripts and item 78 all keep working. The only change is that **the agent assigns the id instead of the app**. `start()` would write `todos.session_id` from the response, not before the call.
3. **The system prompt can still be appended.** `_meta.systemPrompt` as an object is forwarded as `{...obj, type:"preset", preset:"claude_code"}` (lines 9311–9327). `{append: CONTRACT + agent_prompt}` therefore works unchanged. `extraArgs` passes through and is merged (line 9511), so `system-prompt-snapshot: off` survives.
4. **Auto-mode denials are still visible.** The SDK's `permission_denied` becomes a `tool_call_update` with `status:"failed"`, the content `Permission denied: <reason>`, and `_meta.claudeCode.toolResponse.decisionReasonType` (`src/tool-calls/renderer.ts:530`). The app can recognise it and raise attention deterministically. The fallback is `_meta.claudeCode.emitRawSDKMessages: [{type:"result"}]`, which forwards raw SDK messages as `_claude/sdkMessage` notifications, including `result.permission_denials`.

Further adapter facts:

- **Process model.** One adapter process serves many sessions (`this.sessions` map). Each session has its own Claude Code subprocess underneath. Switchboard would run one long-lived `claude-agent-acp` child for all TODOs.
- **Modes.** `default`, `acceptEdits`, `plan`, `auto` and `bypassPermissions` (opt-in) are available as config options. There is no `dontAsk` mode. The adapter sets `permissionMode` itself and ignores it when it comes in `_meta` options.
- **No in-process extras.** In-process hooks and in-process MCP servers cannot cross the process boundary, because `_meta` is JSON and functions do not serialise. The `PreToolUse` and `UserPromptSubmit` callbacks are replaced by `tool_call` and `user_message_chunk` updates.
- **Prompt queueing and steering.** The adapter advertises `_meta.claudeCode.promptQueueing` and a `_session/steering` extension, which inject a message into a running turn. Switchboard does not use this today.
- **No `terminal/*` or `fs/*`.** The adapter does not call the client's terminal or file methods. Bash and file edits run inside Claude Code. Output can be mirrored through a `_meta.terminal_*` opt-in. Switchboard's PTY terminals are therefore unaffected.
- **Usage.** `usage_update {used, size}` covers the context meter (item 123). The model shows in the `model` config option.

## 3. Switchboard concern by concern

| `runner.ts` concern today | ACP v1 equivalent | Adapter | Change for the app |
|---|---|---|---|
| `query({sessionId})` per turn, then `resume` | `session/new` once; later `session/prompt` on a live session; after an app restart, `session/resume` (or `load` to get history) | Yes | The run loop becomes "await `session/prompt`". Its response stays the idle signal (spec 407 still holds in v1). |
| App-generated session id | Agent-assigned `sessionId` | Same UUID as Claude's | Persist it after `session/new` returns |
| `drain()` reading `result` | `stopReason` plus `session/update` | Yes | The final text is the last `agent_message_chunk` run. `duration_ms` is measured by the app. |
| `permission_denied` → app message + attention | `tool_call_update` failed + `_meta.claudeCode.toolResponse` | Yes | Same deterministic rule, keyed on the meta field |
| `PreToolUse` hook → event row, `last_tool` | `tool_call` (`title`, `kind`, `name`, `rawInput`) | Yes | Simpler, and gives tool status and results for free |
| `usageStep` (assistant usage, `compact_boundary`, `contextWindow`) | `usage_update {used,size}`, `compaction_update` (Preview) | Yes | Smaller code. The model comes from config options. |
| `interrupt()`, then abort after 10 s | `session/cancel`, then `session/close` | Yes | Stop and Esc in the UI become cheap to add |
| `systemPrompt` append, snapshot off | `_meta.systemPrompt {append}`, `_meta.claudeCode.options.extraArgs` | Yes (Claude only) | Not portable. Other agents need the contract in the prompt or an `AGENTS.md`. |
| `permissionMode: auto`, app tools in `allowedTools` | Config option `mode=auto`; `allowedTools` should pass through `_meta.claudeCode.options` (inferred from the spread order, not in the adapter's documented list) | Yes | Same behaviour |
| `createSdkMcpServer` "app", per-TODO closure | `mcpServers: [{type:"http", name:"app", url, headers}]` or stdio | http, sse and stdio only | **Biggest change**, see §4 |
| `getSessionInfo` restore check | `session/list` (filter by cwd) or try `session/resume` | Yes | Fine |
| Operator: `tools` whitelist, `strictMcpConfig`, `dontAsk` | `_meta.claudeCode.options.{tools, strictMcpConfig}`; no `dontAsk` mode, so use `default` and auto-reject `request_permission` in the client | Partly | Workable but Claude-specific. The simplest choice is to leave the operator on the SDK. |
| Claude terminals (`claude --add-dir ...`) | Not affected | n/a | None |

## 4. Design: app tools over MCP, out of process

Because ACP keeps MCP for client tools, the app server has to get a transport. Two options:

**4a. HTTP MCP endpoint on the existing Bun server (recommended).**
- Route: `POST /mcp/app` on `127.0.0.1:4477`, using the MCP Streamable HTTP server transport from `@modelcontextprotocol/sdk`, or a small hand-written JSON-RPC handler, since only `tools/list` and `tools/call` are needed.
- Each session gets `mcpServers: [{type:"http", name:"app", url:"http://127.0.0.1:4477/mcp/app", headers:[{name:"X-Switchboard-Session", value:<random per-session secret>}]}]`. The server maps secret → `todoId`, which restores the per-TODO closure from spec 407 without matching by session id.
- Security stays within the current model: loopback only, the Host check, and a capability secret per session instead of the launch token. The global launch token never reaches the agent.
- Tool names are unchanged because the server is still called `app`: `mcp__app__post_message` and so on. The contract text does not change.
- Handlers are the same functions as now, moved from `runner.ts` closures into a module keyed by `todoId`.

**4b. A stdio shim** (`bun mcp-app.ts --todo <id>`), spawned by the agent, which forwards to the REST API with a session secret. It works with every ACP agent, because stdio MCP support is mandatory while HTTP is optional (Codex supports stdio and http; HTTP support in Gemini CLI is unverified). The cost is an extra process per session and one more hop.

Choose 4a for Claude and Codex, and fall back to 4b only for an agent without `mcpCapabilities.http`.

**Side effect worth noting.** An HTTP app MCP endpoint is exactly the missing piece that spec line 471 names for tracking `claude` processes started in terminals. Once it exists, a Claude terminal could get `--mcp-config` pointing at it, and the terminal session would then have `post_message`, `set_attention` and the other app tools too.

**Later: MCP-over-ACP.** When the RFD stabilises and the adapter implements it, the app tools could move onto the ACP connection itself, as `{type:"acp", name:"app"}` with `mcp/message` handled in-process. That would give back "no MCP endpoint" while running out of process. It does not exist yet, so do not design for it now. Keep the tool handlers transport-agnostic so the switch is cheap.

## 5. Design: the driver

```
AgentDriver (one per agent kind)
  open(todo, {cwd, addDirs, appendPrompt, appTools}) -> sessionId
  resume(sessionId, {cwd, ...}) -> ok | gone
  prompt(sessionId, blocks) -> stopReason        // resolves at turn end = idle signal
  cancel(sessionId)
  events: onUpdate(sessionId, update)            // ACP SessionUpdate shape
          onPermission(sessionId, req) -> outcome
```

- `SdkDriver` wraps today's code. It maps SDK messages to ACP-shaped updates, emitting only the few the app uses today.
- `AcpDriver` spawns `claude-agent-acp`, or `codex-acp`, `gemini --acp`, and so on. It uses `@agentclientprotocol/sdk`'s `client()` builder over stdio: one child per agent kind, many sessions each.
- `runner.ts` keeps everything that is not agent-specific: delivery, queueing, hand-over, attention rules and events. That code is already agent-agnostic (`lib.ts:90-95`, `handOver`).

**DB changes.**
- `sessions.agent` (`claude` | `codex` | …) and `projects.default_agent`.
- `todos.session_id` keeps its meaning.
- `ctx` comes from `usage_update`.

**UI possibilities, each optional and each currently on the "not built" list.**
- **Transcript view:** stream `agent_message_chunk`, `tool_call` and `diff` into an Events or Transcript tab. Chat stays the curated channel via `post_message`. This conflicts with the spec's choice that the user sees only posted messages, so keep it behind the existing Events switch.
- **Permission prompts:** `request_permission` → an amber card with the adapter's options. This would allow `default` or `acceptEdits` mode per project instead of `auto` only.
- **Model and effort:** set per project or per TODO through `set_config_option`.
- **Stop button:** `session/cancel`.
- **Plan:** the agent's TodoWrite becomes `plan` updates, which could feed a progress line on the card.

**What stays the same:** the PTY terminals, Files, git and PR handling, the operator (still on the SDK), the artifact folders and the hand-over header.

## 6. Risks and costs

- **Another moving dependency.** The adapter ships often (v0.86, with the SDK pinned inside it) and many features sit behind `_meta` extensions. The app would pin the version and watch for changes.
- **One more layer for Claude-only users.** The adapter is itself built on the same Agent SDK. For Claude alone, ACP adds a process and a translation step and brings no capability the SDK lacks.
- **v2 is coming.** In v2 the prompt response no longer marks the end of a turn; `state_update` does. The idle and queue state machine would need adjusting. Design against v1 now, and keep "turn ended" as one event inside the driver so v2 only touches the driver.
- **Agents differ.** Agents vary in system-prompt control, resume behaviour and MCP-over-HTTP support. The contract has to work as plain prompt text for agents with no append hook.
- **Item 78 and similar.** Agents other than Claude have no `claude --resume`, so the "resume outside the app" hints become specific to each agent.

## 7. Options and recommendation

| Option | What | Effort | Gains |
|---|---|---|---|
| A. Status quo | Keep the SDK and in-process MCP | 0 | Nothing |
| B. Full switch | ACP client for everything, HTTP app MCP endpoint, operator on ACP | Large | Multi-agent, streaming, permissions; loses in-process simplicity |
| **C. Seam first** | `AgentDriver` interface shaped like ACP; `SdkDriver` first; move tool handlers into a transport-agnostic module | Small to medium | Makes B incremental; nothing on the not-built list changes |
| C → B | Add `AcpDriver` + `/mcp/app` once multi-agent or a live transcript is wanted; Claude can keep `SdkDriver` | Medium | Codex and Gemini as worker agents; Claude terminals with app tools |

**Recommendation: C now, then add the ACP driver when a second agent or a live transcript becomes a real goal.** The adapter checks in §2 show nothing that blocks B. The real decision is product scope: whether to give up the deliberate choices behind "not built on purpose" (no permission UI, no model setting, curated chat only).

## Questions for you

1. Is the motivation **other agents** (Codex, Gemini …), **a richer live view** of what the agent does, or **in-UI permissions**? Each one points to a different first step.
2. Should Claude itself move to ACP (one code path), or stay on the SDK while other agents use ACP (fewer layers for the common case)?
3. Is an HTTP MCP endpoint on the loopback server acceptable now, given that the spec avoided it on purpose?

Answers:
1: all mentioned
2: Lets move claude as well, so the answer in 7 is B, full switch
3: do the full move as required
