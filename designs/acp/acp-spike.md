# ACP spike (P0): findings

Date: 2026-10-07. Code: `spike/acp/` (`harness.ts`, `checks.ts`, `latency.ts`, `smoke.ts`, `sdkpush.ts`).
- Run with `cd spike/acp && bun install && bun checks.ts [check …]`.
- Results go to `spike/acp/out/results.json`, raw ACP traffic to `out/log.jsonl`, and adapter stderr to `out/<label>.stderr`.

Setup:
- Agent: `@agentclientprotocol/claude-agent-acp` 0.86.0, started as `bun …/dist/index.js`. It bundles Claude Agent SDK 0.3.287 and the user's Claude Code 2.1.292.
- Client: `@agentclientprotocol/sdk` 1.7.0, using `client().onRequest(...).onNotification(...).connect(ndJsonStream(...))`.
- App MCP: `@modelcontextprotocol/sdk` 1.32.1, `WebStandardStreamableHTTPServerTransport` in stateless mode (`sessionIdGenerator: undefined`, `enableJsonResponse: true`). It runs inside `Bun.serve` on `127.0.0.1:4499/mcp/app` and authenticates each session by the `X-Switchboard-Session` secret.
- Working directory: a throwaway git repo in `/tmp/sb-acp/repo`.
- Runtime: Bun 1.4.2 on Linux arm64. **There is no Node on this machine.**

## Results

| # | Check | Result |
|---|---|---|
| 1 | Adapter runs under Bun | **Pass.** It needs no Node. Spawn plus `initialize` takes 130–150 ms, and `session/new` 550–590 ms. |
| 2 | ACP TS SDK client under Bun | **Pass** |
| 3 | Loopback HTTP MCP with a custom header, through the adapter | **Pass.** Tools are named `mcp__app__set_title` and `mcp__app__post_message`, and every call carried that session's secret. The CLI sends `server/discover`, `initialize`, `notifications/initialized`, a `GET` (answered 405, which is fine for stateless), `tools/list` and `tools/call`. Bare `allowedTools` entries auto-approve the app tools. The adapter logs a `CLAUDE_SDK_CAN_USE_TOOL_SHADOWED` warning for them, which is intended. |
| 4 | `_meta.systemPrompt.append` and `extraArgs` `system-prompt-snapshot: off` | **Pass.** The appended text was seen. After `session/resume` with an edited append, the agent answered with the new text, so the snapshot-off flag reaches the CLI. |
| 5 | ACP `sessionId` is the Claude session id | **Pass.** `~/.claude/projects/-tmp-sb-acp-repo/<id>.jsonl` exists, and `claude --resume <id> -p …` answered from that conversation. |
| 6 | Auto-mode denials | **Pass.** No `request_permission` is sent. Each denial arrives as a `tool_call_update` with `status: "failed"`, content `Permission denied: …`, and `_meta.claudeCode.toolResponse.decisionReasonType`:<br>• `subcommandResults` for a settings deny rule (`Bash(rm:*)`);<br>• `classifier` for `curl … \| sudo bash`, with `decisionReason` filled in.<br>The command itself comes from the tool call's `rawInput.command` or `title`. |
| 7 | `session/cancel` during a running Bash command | **Pass.** `session/prompt` returned `stopReason: "cancelled"` 13–21 ms after the cancel notification. The command's process and its shell wrapper were both gone a second later, checked with `pgrep -af` before and after. The next prompt in the same session works. The agent sees the command as rejected, as with today's `interrupt()`. |
| 8 | Kill the child, then `session/resume` from a new child | **Pass.** It took 613 ms, with no replayed updates. The conversation was kept (it remembered "4711"), and the new MCP secret was used for the app tool call. |
| 9 | `usage_update` and `configOptions` | `usage_update {used, size, cost{amount, currency}, _meta{"_claude/model"}}` came about 9 times per short turn (e.g. `used` 24532, `size` 1000000). Config ids:<br>• `mode`: default, acceptEdits, plan, auto, bypassPermissions;<br>• `model`: default, opus, fable, sonnet, haiku, plus full ids;<br>• `effort`: default, low … max;<br>• `fast` (`model_config`): on, off.<br>**A new session starts in mode `default`**, so the app has to set `auto` explicitly. |
| 10 | `request_permission` in `default` mode | **Pass.**<br>• Write: `kind: edit` with diff content `{path, oldText: null, newText}` and options `allow-once`, `allow-with-updates` (allow_always, "allow all edits during this session") and `reject`.<br>• Bash: `kind: execute` with `rawInput.command` and options `allow-once` and `reject` only; **there is no allow_always for Bash**.<br>Allow and reject both did what they say. |
| 11 | Latency against SDK `query()` | **No penalty.** Time to the first text of "Reply with just the word hi": ACP 2.3 s including spawn; SDK `query()` 2.9–4.9 s. |
| 12 | The user's config loads | **Pass.** The agent saw the CLAUDE.md "advisor" rule and 38 skills. It also mentioned that the claude.ai connectors need authorization, which shows the user's MCP servers loaded too. |
| 13 | A message while a turn runs (item 20) | **Mixed, see below** |
| op | Operator restrictions | **Pass.** `_meta.claudeCode.options {tools: [Read, Grep, Glob, WebSearch, WebFetch], allowedTools, strictMcpConfig: true}` left only those built-ins, the app MCP tools and `advisor`. The user's MCP servers were not loaded. Asked to create a file, the agent declined because it has no tool for it. No permission request was needed. |

## Item 20 (pushing a message into a running turn) under ACP

There are two ways to send a message while a turn runs:

- **A plain `session/prompt` while a turn runs** (the adapter advertises `_meta.claudeCode.promptQueueing`). The second prompt waits until the first turn ends, then runs as its own turn, and both return `end_turn`. That is today's *queue* behaviour, not today's *push*.
- **`_session/steering {sessionId, prompt}`** (advertised in `initialize._meta.steering`). It returns `{outcome: "injected"}`, and the agent reads the message in the same turn, as today's push does. Two problems showed up:
  1. **Steering while a tool call is still streaming** (right after its first `tool_call` update, before execution starts) leaves that tool call `failed`. The `session/prompt` then **rejects** with JSON-RPC error "Claude ended the turn without returning results for tool calls: toolu_…". This reproduced 2 of 2 times. The agent's text was complete and correct, and later prompts in the session work.
  2. **Steering while Bash is running** (4 s into a 10 s command) gives no error. But the turn ended right away: the CLI had moved the foreground command to the background, and the agent said it would report when the command finished.
- **For comparison, today's SDK push** (`sdkpush.ts`): a streaming-input message written 4 s into the same command. The command ran to the end, and the agent read the message after the tool result and answered both ("first done. KIWI") in the same turn. So the item 20 hint ("reads your message after its next tool call") is true for the SDK. **Steering behaves differently: it pre-empts the running tool.**

For P1:
- **Push.** Neither ACP path matches item 20 today: queueing waits for the turn to end, and steering pre-empts. Options:
  - (a) Use steering and change the item 20 hint (a running command goes to the background).
  - (b) Use queued `session/prompt` and change item 20 to "answered when the turn ends".
  - (c) Ask upstream for a steer that waits for the next tool boundary. The adapter already has an internal `later` priority, but it is not on the wire.

  This is a product decision for P1.
- **Steering error.** If steering is used, handle the error case without matching the adapter's error text. When the prompt rejects after a steer, agent text has arrived since the steer, and no tool call is still pending or in progress, settle the turn as `end_turn` with an `error` event and no chat message.
- **Upstream report (P1 task).** Report both problems to agentclientprotocol/claude-agent-acp: the error when steering during tool-call streaming, and the pre-empting of a running tool.

### Re-run on 2026-10-11 (Claude Code 2.1.296)

The same check, with the agent running two 6 s Bash commands in a row and a steer at three moments:

- **While the first tool call streams.** On 0.86 the prompt still rejects. On 0.89.1 it returns `end_turn`: the half-written call is "stopped before it ran", and the agent issues it again (2 of 2 runs).
- **3 s into the running Bash.** The command moves to the background on both versions. On 0.89.1 the agent waited for it once and ended the turn early once; on 0.86 it ended the turn early.
- **Right after a tool call completed.** On 0.89.1 the result was clean in 2 of 2 runs: the agent read the message in the same turn and nothing was backgrounded.

Item 20 now steers at that third moment, on 0.89.1 (see `acp-design.md`).

## Consequences for the design (`acp-design.md`)

- **Packaging.** It needs nothing new: the adapter runs on the Bun that already runs the server. Bun's own node compatibility is enough.
- **Mode.** `session/new` must be followed by `set_config_option mode=auto`, or the project's mode. The `effort` and `model` defaults are applied the same way.
- **Denial messages (item 50).** These keep their wording, "Auto mode blocked Bash: <rawInput.command>", keyed on `status: failed` plus `_meta.claudeCode.toolResponse.decisionReasonType`. That also answers the spec's open question (line 512): **the classifier does deny** (`curl … | sudo bash`).
- **Permission UI.** It must handle option sets without allow_always (Bash), and the edit case with a diff preview.
- **Stop (item 129).** This is `session/cancel`. The 10 s fallback stays, but the spike never needed it. It should send SIGTERM to the adapter child, not SIGKILL, so the adapter can stop its Claude CLI grandchild.
- **Untested here, to check in P1:**
  - Item 129's "a pushed message the agent has not read is answered right after the interrupt" under steering plus cancel.
  - `additionalDirectories` (the worktree root and the artifact folder).
- **The adapter's stderr** is useful for debugging: it logs `[session/create]` timing and turn errors. The app should keep its last lines per TODO.

## P4 probe: config options and slash commands (2026-10-07)

`spike/acp/config-probe.ts` runs the app's own driver against claude-agent-acp.

- Config ids: `mode` (category `mode`), `model` (`model`: `default`, `opus`, `fable`, `sonnet`, `haiku` and full ids), `effort` (`thought_level`: default, low … max), `fast` (`model_config`: on, off), `agent` (no category: default and the user's subagents).
- **A model switch rewrites the other options.** `haiku` drops `effort` and `fast` and sets the mode to `acceptEdits`; setting `auto` afterwards returns no error but the mode stays `acceptEdits`. Back on `opus`, effort and fast return and the mode stays where it was.
- The switch takes effect on the next prompt of the same session (the agent answered `claude-haiku-4-5-20251001`, then `claude-opus-5-5`), and survives a `session/resume` in a new process.
- `available_commands_update` arrives right after `session/new`: 106 commands with `name`, `description` and `input.hint`.
