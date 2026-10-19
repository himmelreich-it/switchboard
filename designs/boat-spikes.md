# boat.dev spikes

Status: PROPOSED · 2026-10-19 · no code changed. Line numbers checked against main `ae6107d`.

## Why

[boat.dev](https://boat.dev) rents Linux VMs ("sandboxes") by the second: 4 vCPU / 8 GB / 50 GB for $0.036/hour,
nothing while stopped. Claude Code, Docker, Chrome, Bun, Node and `gh` are preinstalled. The CLI that matters here:

| Command | Does |
|---|---|
| `boat new [--ttl s] [--no-env -e K=V]` | starts a VM, prints its id (`bx_…`); auto-stops 1 h after start by default |
| `boat exec <id> [--cwd d] -- cmd` | runs a command, pipes stdin/stdout, exits with its code (timeout 1–600 s) |
| `boat ssh <id> [cmd]` | shell or one command; `boat info <id>` gives the IP for plain `ssh` |
| `boat scp [-r] src bx_<id>:/path` | copies files |
| `boat host <id> <port>` | public HTTPS URL for a port on the VM |
| `boat fork <id>` | second VM with the same disk, usable in a few seconds |
| `boat extend <id> --hours n` / `boat stop` / `boat delete` | lifetime |

Two ways to use it from switchboard, cheapest first. Each spike answers "does it work, and what breaks" in at most
half a day. Neither ships code into `main`; spike B uses only a Settings row and a hand-built VM.

Open before either spike: Hostaway code and the Claude subscription token on a third-party EU VM may not be allowed.
Both spikes use the switchboard repo (personal account) so that question does not block them.

## Today

```
laptop
┌──────────────────────────────────────────────────────────────┐
│ bun server.ts                                                │
│   runner.ts:334 agentFor()                                   │
│     └─ acp.ts:67 Bun.spawn(spec.command)   ACP over stdio    │
│          = bun claude-agent-acp   (agents.ts BUILTINS)       │
│            cwd = project local_path       (runner.ts:616)    │
│            calls back http://127.0.0.1:4477/mcp/app          │
│              (apptools.ts:35) with X-Switchboard-Session     │
│   file browser, terminals, diff, PR card read worktree_path  │
│     on local disk (14 uses in server.ts, 2 in runner.ts)     │
└──────────────────────────────────────────────────────────────┘
```

## Spike A: a worker sends a heavy job to a boat VM

Switchboard does not change. The worker stays on the laptop with the user's full config and calls the `boat` CLI
from Bash, the way it calls `gh` today.

```
laptop                                     boat VM (throwaway)
┌────────────────────────────┐  boat new   ┌──────────────────────────────┐
│ worker in a TODO           │────────────▶│ bun install && bun test      │
│  worktree on local disk    │  boat scp   │ bun server.ts (test home)    │
│                            │  boat exec  │ Playwright + preinstalled    │
│                            │◀────────────│   Chrome against it          │
│ reads output, boat delete  │  stdout     │ boat host 4477 → https URL   │
└────────────────────────────┘             └──────────────────────────────┘
```

### Steps

1. Install the CLI and sign in by hand: `curl -fsSL https://boat.dev/install | sh`, `boat onboard`.
   The 7-day trial gives 25 hours and caps a VM at 2 hours.
2. Start a switchboard TODO whose text says: "Run `bun test` and a Playwright smoke test of this branch on a boat
   VM, not on the laptop. Delete the VM when done." No project prompt change, no skill yet.
3. Expected worker commands (written here to compare against what it does):
   ```sh
   id=$(boat new --no-env --ttl 3600 --json | jq -r .id)
   git bundle create "$TMPDIR/sb.bundle" HEAD
   boat scp "$TMPDIR/sb.bundle" "$id:/home/user/sb.bundle"
   boat exec "$id" -- bash -lc 'git clone sb.bundle sb && cd sb && bun install && bun test'
   boat exec "$id" --cwd /home/user/sb -- bash -lc 'SWITCHBOARD_HOME=$HOME/sbhome bun server.ts & sleep 3; …playwright…'
   boat delete "$id" --yes
   ```
   `--no-env` keeps the user's GitHub token and model keys off the VM; the bundle means no push is needed.
4. Optional: `boat host "$id" 4477` and open the URL. Unknown whether `boat host` reaches a server bound to
   `127.0.0.1` (server.ts:19); if not, note it and move on.

### Measure

- Wall time and boat cost (`boat usage <id>`) against the same run on the laptop.
- Does the worker find and use `boat` from the TODO text alone, or does it need a skill?
- Does `boat exec`'s 600 s timeout cut long jobs? If yes, `--detach` plus polling is the pattern to write down.
- What went wrong with credentials, paths, or cleanup (VMs left running).

### Success

`bun test` and one Playwright check pass on the VM, the worker reports the result in chat, the VM is deleted, and
the steps fit into a short skill. Then the follow-up is that skill (or a project prompt line), still no
switchboard code.

## Spike B: switchboard drives an agent that runs on the VM

The agent process moves to the VM; switchboard keeps talking ACP to it. This uses the existing custom-agent rows
(`POST /api/agents`, server.ts:1238), so no code change is needed for a first look.

```
laptop                                              boat VM
┌─────────────────────────────────────┐  one ssh    ┌───────────────────────────────┐
│ acp.ts:67 Bun.spawn(                │  process    │ claude-agent-acp              │
│   ssh -R 4477:127.0.0.1:4477        │════════════▶│  ACP on stdin/stdout          │
│       user@<ip> claude-agent-acp)   │  stdio +    │  cwd /Users/oskar/code/       │
│                                     │  tunnel     │      switchboard (mirrored)   │
│ 127.0.0.1:4477/mcp/app ◀────────────│─────────────│  → 127.0.0.1:4477 on the VM   │
└─────────────────────────────────────┘             └───────────────────────────────┘
```

One plain `ssh` process carries both the ACP stdio and the reverse tunnel, so `boat forward --reverse` is not needed.

### Why a custom agent row and not the built-in Claude

A custom row's id is not `claude`, so `isClaude()` is false (agents.ts). That changes two things in our favour:

- `secretHeader()` (agents.ts:82) puts the MCP secret itself in the header, sent over ACP. The built-in Claude gets
  `${SWITCHBOARD_MCP_SECRET}`, which the VM's process would not have.
- The contract and project prompt go as text in front of the first prompt (runner.ts:708) instead of the Claude
  `_meta` system-prompt append (runner.ts:633). Fine for a spike; worse prompt placement.

### Steps

1. VM: `boat new --no-env -e CLAUDE_CODE_OAUTH_TOKEN=<from claude setup-token> --ttl 7200`, then on the VM:
   ```sh
   npm i -g @agentclientprotocol/claude-agent-acp@0.86.0      # same version as package.json
   sudo mkdir -p /Users/oskar/code && sudo chown user /Users/oskar/code
   git clone https://github.com/himmelreich-it/switchboard.git /Users/oskar/code/switchboard
   mkdir -p /Users/oskar/code/worktrees /Users/oskar/.switchboard/data
   ```
   The mirrored paths exist because runner.ts:616 sends the laptop's `local_path` as the session cwd and
   runner.ts:631 adds the worktree root and artifact folder as extra directories. Whether `sudo` works on a boat
   VM is unverified.
2. Laptop: `boat info <id>` for the IP and key, then add a custom agent:
   ```sh
   curl -X POST "http://127.0.0.1:4477/api/agents" -H "X-Switchboard-Token: $(cat ~/.switchboard/token)" \
     -d '{"name":"Claude on boat","command":"ssh","args":["-i","/Users/oskar/.ssh/ascii_box_ed25519",
          "-o","StrictHostKeyChecking=accept-new","-R","4477:127.0.0.1:4477","user@<ip>","claude-agent-acp"]}'
   ```
   The token header is checked at server.ts:1362. The key path is spelled out because `Bun.spawn` does not expand
   `~` in arguments.
3. Create a TODO in the switchboard topic with agent "Claude on boat". Ask it to set a title, post a message, and
   edit a file.

### Expected breaks (record each, do not fix in the spike)

| What | Why | Fix size if we go on |
|---|---|---|
| Files the agent writes do not show in the file browser, diff, or terminals | they read `worktree_path` on local disk (14 uses in server.ts) | large: a remote file layer or a sync |
| `attach_file` fails or attaches nothing | the artifact folder the agent writes to is on the VM; the server reads the laptop's | medium: copy back with `boat scp` on attach |
| One VM is wired into the agent row, not per TODO | `specOf()` (agents.ts:38) returns one fixed command per agent | small: `todos.sandbox_id` and a `{sandbox}` placeholder |
| VM stops mid-turn | TTL counts from start; boat only extends it for its own `boat prompt` | small: `boat extend` on each turn start, or `--no-auto-stop` (needs a payment method) |
| Agent dies when the laptop sleeps or the app quits | the ssh process is the server's child, as in `keep-agents-running.md` | not fixable in this design |
| No user hooks, skills, CLAUDE.md, claude.ai connectors | the VM has none of `~/.claude` | medium: copy into a boat environment; connectors I don't know |

### Measure

- Does `initialize` and `session/new` succeed over ssh, and how long does the first turn take against local?
- Do the app tools (`set_title`, `post_message`) arrive through the reverse tunnel?
- Does a permission request reach the UI?
- Does reopening the session after the ssh process ends work (`session/resume` on the VM)?

### Success

A TODO runs on the VM: title set, message posted, file edited on the VM, and the break table above is confirmed or
corrected. The result decides between stopping at spike A and a design for per-TODO VMs.

## Not in either spike

Boat's own agent server (`POST /sandboxes/{id}/prompt`, `boat events`, `boat steer`). It keeps agents running with
the laptop closed, but it is a second runner beside ACP without permission requests, config options or slash
commands. Revisit only if "agents run with the laptop closed" becomes a goal.
