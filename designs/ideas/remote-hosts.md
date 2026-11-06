# Remote hosts: design

Status: IDEA, for later · 2026-11-04 · not built. The browser on the network (`designs/browser-on-lan.md`) was built instead. Line numbers are from main `f95e65b`.
It grew out of two earlier notes (an SSH option and a daemon per machine), which it replaces.
It is meant to carry T-15 (sandboxed sessions, `2026-11-03-sandboxed-sessions-design.md`) as a second transport.
A reviewer read the first draft; its 15 findings are folded in (marked ⚑ where they changed the design).

## Why

You work on several home machines that you already reach with `ssh` over Tailscale. You want one Switchboard, on the machine in front of you, that runs a topic's agents, git, files and terminals on another machine.

Running a daemon per machine and browsing to it works, but it puts a shell-granting token on the network. Here nothing new listens on the network: the only way in is the `ssh` you already trust.

## The model: VS Code Remote

VS Code splits into a local UI and a **VS Code Server** that it installs on the far side. Remote-SSH, Dev Containers and WSL are only different **transports** to that same server: a pipe over `ssh`, over `docker exec`, or over `wsl.exe`. Extensions, the file system, terminals and port forwards all run in the server and talk to the UI over the one pipe.

Switchboard copies that split:

```
this machine (Windows laptop)                               far side (desk, Linux)
┌───────────────────────────────────────┐                   ┌──────────────────────────────────────────┐
│ bun server.ts   UI, db, operator      │  one pipe per     │ sb-remote   (helper, Bun script)         │
│  └─ Host "desk" (SshHost)             │  host:            │  exec / spawn / pty / fs / git ops       │
│      ssh -T -- desk /bin/sh -c …      │═══════════════════│  MCP relay on 127.0.0.1:<random>         │
│  └─ Host "local" (LocalHost) = today  │  framed RPC on    │  └─ claude-agent-acp  (ACP via spawn)    │
│  └─ later: "box t15" (ContainerHost)  │  stdio            │  └─ git, shells, the topic folder        │
└───────────────────────────────────────┘                   └──────────────────────────────────────────┘
```

- **Host**: where a topic's work happens. There is always a `local` host (today's behaviour). You add others in Settings › Hosts.
- **Transport**: how the pipe is made. `ssh` (phase 1), `docker exec` (T-15), `wsl.exe` (later).
- **sb-remote**: one script, the same for every transport. It does the work on the far side and answers over the pipe.

## Goals

1. A topic can live on another machine. Its agents, git, file browser, editor, diff and terminals run there, and the UI looks the same as for a local topic.
2. Nothing listens on the network. The pipe is `ssh` with your keys, your `~/.ssh/config` and Tailscale SSH or MagicDNS, unchanged.
3. Adding a host takes one field (the ssh alias) and one **Test** click. The helper installs itself.
4. When a host is offline, its topics say so. Switchboard never falls back to running on this machine.
5. **A compromised far side cannot reach this machine** beyond the TODOs of its own topics. ⚑
6. The same host layer is what T-15's containers plug into.

## Non-goals (phase 1)

- Agents that survive a dropped connection or a sleeping laptop. Covered under [Later](#later).
- One TODO spread over hosts, or moving a TODO between hosts.
- Syncing your `~/.claude` setup to the far side. It is shared with T-15 Q5; phase 1 uses whatever the far side has.
- Windows as the far side. The far side is Linux or macOS.
- Port forwards, the Ports view and Reveal for remote topics.

## Security model

The far side is treated as **less trusted than this machine**. That is the same stance T-15 needs for a box, so one set of rules serves both.

| Concern | Answer |
|---|---|
| Who can reach the far side | Whoever can `ssh` there, as today. Switchboard opens no port on any network. |
| The ssh target | Validated (no leading `-`, no spaces or control characters) and passed after `--`, so a crafted alias cannot inject `-oProxyCommand=…`. ⚑ |
| Host keys | `ssh -o BatchMode=yes -o ConnectTimeout=5`, never `StrictHostKeyChecking=no`. An unknown key fails with "Run `ssh desk` once in a terminal and accept its key". Tailscale SSH in *check* mode also fails under BatchMode, and Test says so. |
| **App tools called from the far side** ⚑ | Every grant records its host. The relay rejects frames whose session secret belongs to another host. For a remote grant, `read_todo`, `read_file`, `list_todos` and `list_comments` see only TODOs of topics on the same host. `create_todo` may target only that host's topics; for other topics it is allowed only with `start: false`. Otherwise a compromised desk could read the laptop's TODOs and uploads, or start an agent on the laptop with its own instructions (today apptools.ts:259-339 allows any `todo_id` and topic). |
| **Far-side strings reaching a local shell** ⚑ | Never. Paths from the far side (a worktree path the agent set, file names) never go through `cmd.exe`, `sh` or `vscodeCommand` (lib.ts:603 builds a verbatim `cmd.exe` line, where `/x" & calc & "` is a legal Linux folder name). VS Code opens through a `vscode://` URI ([UI](#ui)). |
| **Files written on this machine from far-side data** ⚑ | Only into the artifact cache, under strict name rules ([Artifacts](#artifacts)). |
| The MCP relay on the far side | It listens on `127.0.0.1:<random>` there. Each agent session keeps its own secret, as today on this machine. Other users on that machine get 403. |
| The launch token / `/api/` | It never leaves this machine. |
| The pipe | Frames are capped (1 MB), stream totals are capped per kind, data for unknown channels is dropped, and control frames (ping, cancel) jump the queue. A bad far side cannot exhaust memory, and a big transfer cannot stall pings or keystrokes. ⚑ |
| Environment | Nothing from this machine's environment goes to the far side. `acp.ts:67` and `spawnTerm` merge `process.env` today; `host.spawn` uses the far side's own login environment plus the few variables Switchboard sets (`SWITCHBOARD_MCP_SECRET`, `SWITCHBOARD_TODO_ID`…). ⚑ |
| Helper integrity | Uploaded to `~/.sb-remote/<content-hash>/`. `hello` reports the hash, and a mismatch reinstalls. |
| Credentials on the far side | Claude login and `gh` login live there (the agent pushes from there). |

Compared with the daemon idea: no token in a URL, no HTTPS endpoint, no tailnet ACL to get right.

## Data model

```sql
CREATE TABLE hosts (
  id         INTEGER PRIMARY KEY,
  name       TEXT NOT NULL UNIQUE,   -- "desk"; shown on chips
  kind       TEXT NOT NULL,          -- 'ssh' only in phase 1; 'container' / 'wsl' added with those transports
  target     TEXT NOT NULL,          -- ssh alias ("desk", "oskar@nas"), validated
  facts      TEXT,                   -- JSON from the last hello: os, arch, home, login PATH, bun path + version, git, gh, claude login
  checked_at TEXT
);
ALTER TABLE projects ADD COLUMN host_id INTEGER REFERENCES hosts(id);   -- NULL = this machine
```

- `projects.local_path`, `todos.worktree_path` and `places` keep their names. They now mean "a path **on the topic's host**".
- **Host lock** ⚑: a topic's host and folder are read-only once a TODO has a session (the rule at server.ts:1128 extends to the host). **Change topic** (`changeRepo`, runner.ts:1079) to a topic on another host is refused in phase 1 with "T-12 runs on desk; this topic is on this machine". Moving a TODO's artifact folder and places across machines is its own feature.
- **Install id**: each Switchboard data folder gets a random `install_id` (in the db). Everything on the far side lives under `~/.sb-remote/<install-id>/`, so the regular app and Beta (separate data since #25), or two laptops, never share TODO folders on desk. ⚑

## The host interface

All topic work goes through one object per host. `LocalHost` is what the code does today, moved behind the interface:

```ts
type HostPath = string & { readonly host: unique symbol };   // a path on some host; only host.* accepts it

interface Host {
  id: number | null; name: string;
  path: typeof posix | typeof win32;                   // path rules of the far side: join, resolve, isAbsolute, ~
  state(): "connected" | "connecting" | "offline";     // + last error
  exec(cmd: string[], o?: { env?, timeout?, cwd? }): Promise<{ ok, code, out, err }>;    // replaces sh() for topic work
  spawn(cmd: string[], o: { env?, cwd? }): Proc;       // Bun.Subprocess-like: stdin, stdout, stderr, exited, kill()
  pty(cmd: string[], o: { cwd, env, cols, rows, data }): Pty;
  fs: {
    stat, lstat, readdir, readFile(max), writeFile(expectMtime), realpath, exists, mkdir,
    resolveIn(roots: HostPath[], rel: string, write: boolean): Promise<HostPath>,   // fsTarget, run on the far side
    walk(root, cap),                                                               // plainFiles
  };
  git: { states(dirs[]), tree(dir, base), show(dir, sha, path), root(dir), isRepo(dir), worktreeRoot(dir), remote(dir), scan(dirs) };
  agents(): Promise<AgentInfo[]>;                      // which agents are installed there (specOf / onPath on the far side)
  mcpUrl(): Promise<string>;                           // where this host's agents reach /mcp/app
  toFar(localPath): string;                            // artifact folder and uploads: local path → far-side path
}
```

- **Chatty operations go to the far side whole.** `git.states` takes *all* of a host's worktrees in one call (status plus ahead/behind for each). `git.tree` is three git calls, `git.scan` is the Settings › Worktrees scan, and `fs.walk` is thousands of `lstat`s. The helper runs each in one round trip, using the same pure parsers from `lib.ts` (`parseGitStatus`, `parseRefCounts`, `parseWorktreeList`…), which are bundled into the helper.
- **`resolveIn` runs on the far side** with that side's `realpath` and path rules. Today's containment and symlink checks (server.ts:838-852) move into it unchanged.
- **Path rules** ⚑: a Windows server must never apply `node:path` or `homedir()` to a far-side path:
  - `set_worktree` stores `resolve(path)` (apptools.ts:149), which turns `/home/x` into `C:\home\x`.
  - `findWorktreeRoot` (runner.ts:674) and `expandHome` (server.ts:818) expand `~` with the local home.
  - `join(dir, "terminal")` writes backslashes.

  The `HostPath` brand makes these type errors. Phase 1a adds a test that greps `server.ts`, `runner.ts` and `apptools.ts` for `node:fs`, `existsSync`, `realpathSync` or `sh(` applied to topic, worktree or place paths. On a Mac or Linux laptop a missed call site would otherwise *silently act on the local repo*, because the same path often exists on both machines.

### Call sites

| Area | Today | Becomes |
|---|---|---|
| git via `sh()` | server.ts:230 plus about 25 callers, `git -C <dir>` | `hostOf(topic).exec` / `.git.*` |
| direct git spawns | runner.ts:86, runner.ts:671, server.ts:1389 | `host.git.isRepo` / `.worktreeRoot` / `.show` |
| **GitHub queries** ⚑ | `fetchPrs` runs `git remote get-url` in `local_path` and `localOnly` runs `git log` (server.ts:332, 398); `projectOut` runs 2 git calls per topic on every GET /api/projects (server.ts:686); the worktree scan runs git, `du` and `realpathSync` (server.ts:410-471) | git parts through `host.git.remote` / `.scan`. `gh` itself stays local. The repo name is cached per topic, and offline hosts are skipped. |
| **list git state** ⚑ | `summaries` awaits `cachedGitState` per unseen worktree; the cache and `todos WHERE worktree_path = ?` are keyed by path alone (server.ts:264-299) | keyed by (host, path). A remote host's rows are **never awaited**: the list returns "—", one batched `git.states` runs, and the values arrive over SSE. `/api/todos`, the operator's `list_todos` / `read_todo` and `writeContext` never wait on ssh. |
| file browser, editor, diff, comments | `rootDir`, `fsTarget`, `fileFacts`, `plainFiles` (server.ts:833-891), comments (server.ts:1311) | `host.fs.*` |
| **agent spawns** ⚑ | `acp.ts` spawns for session open and resume, `sessionExists` (restore, server.ts:1563, with local `existsSync(folder)`), `describe`, `authenticate`, `scanRepo`, routing | `acp.ts` takes a spawner: `host.spawn`. `specOf` / `onPath` / `agentList().found` per host (`host.agents()`), so "installed" and custom agents are the far side's. |
| **agent login** ⚑ | Settings › Agents › Log in, and the `AuthRequired` message pointing there, log in on this machine | per host: ACP `authenticate` through `host.spawn` ("Log in on desk") |
| MCP URL | `grant()` hard-codes `127.0.0.1:4477` (apptools.ts:31) | `grant(…, host, await host.mcpUrl())`; the grant carries the host id |
| terminals | `TERM_UNSUPPORTED` check, then `Bun.spawn(… terminal)` (server.ts:963), `killTree`, `termBusy` | the Bun-version check applies to the host that runs the PTY; `host.pty`; kill and busy are RPCs |
| path checks on agent-supplied paths | `existsSync` in set_worktree (apptools.ts:148), restore, VS Code, terminal root, routing dirs (runner.ts:989), `swapPlace` (runner.ts:1079) | `host.fs.exists` on the host of that path |
| topic folder validation | `requireFolder`, `expandHome` (server.ts:815-821) | `host.fs.stat`, with `~` expanded by the far side |
| **local paths in prompts** ⚑ | see [Artifacts](#artifacts) | `host.toFar` |
| VS Code | `vscodeCommand` (lib.ts:603) | a `vscode://vscode-remote/ssh-remote+<alias><url-encoded path>` URI, opened by the page |
| Reveal, Ports view | `revealCommand`, `lsof` | off for remote topics |

Everything that is not topic work stays on `LocalHost` or plain `node:fs`: the database, the artifact cache, uploads, the operator, the token and static files.

## sb-remote, the helper

- **What**: `remote.ts` in this repo, bundled with `bun build --target=bun` into one file of about 100 KB. It contains the RPC loop, the fs/git/exec/pty operations, the MCP relay, `killTree`, and the pure parts of `lib.ts` it needs.
- **Runtime**: Bun ≥ 1.3.5 on the far side (PTYs need it). Phase 1 requires Bun; Test says how to install it ([Q1](#q1)).
- **Agent adapter**: on the far side, `bun install --frozen-lockfile` in `~/.sb-remote/<hash>/`, with a `package.json` and `bun.lock` generated from this repo's lockfile. That pins the adapter at the same version this machine runs (0.89.1 today). Its SDK's optional platform package brings the right native `claude` for that OS and architecture. This machine's `node_modules` only has the Windows ARM64 one.
- **Bootstrap** ⚑: one ssh call, which also gives one handshake per connect:

  ```sh
  ssh -T -o BatchMode=yes -o ConnectTimeout=5 -o ServerAliveInterval=15 -- desk "exec /bin/sh -c '<bootstrap>'"
  ```

  The bootstrap is a short POSIX script sent on stdin, so fish and csh login shells don't matter. It does three things:
  1. It reads the login and interactive environment once: `$SHELL -lic 'printf <marker>; env -0'`, the same trick as `loginShellEnv` (main.js:12). A plain `ssh host cmd` gets neither a login nor an interactive shell, and Bun's installer puts `~/.bun/bin` in `.bashrc` / `.zshrc`. The absolute `bun` path falls back to `~/.bun/bin/bun`.
  2. It reports facts (OS, architecture, home, Bun version, helper hash present or not).
  3. On request it reads the helper and lockfile from stdin, runs `bun install`, then `exec`s the helper with that environment.

  The helper writes a magic sync marker before its first frame, so a motd, a conda banner or anything else an rc file prints is skipped and cannot corrupt the frame stream.
- **Protocol**: length-prefixed binary frames on stdin/stdout: `[u32 length ≤ 1 MB][u8 type][u32 channel][payload]`. The types are request, response, stream data, stream end, cancel and ping. Requests carry JSON, and stream data stays raw bytes. Large reads stream in chunks with a total cap (`readFile(max)` applies to raw reads too). Control frames are sent first. stderr carries the helper's own log, kept in a ring buffer for **Host › Log**.
- **MCP relay**: on `hello`, the helper opens `127.0.0.1:0` on the far side. Each HTTP request to it becomes a frame. This side checks the session secret, **checks the grant's host matches** ⚑, and passes the request to `handleMcp(req)` (apptools.ts:41). `host.mcpUrl()` returns `http://127.0.0.1:<port>/mcp/app`. This replaces `ssh -R`: no fixed port, so no clash between two laptops or two hosts.
- **Process ownership** ⚑:
  - `ssh -T` gives no pty, so sshd sends no SIGHUP when the pipe drops, and the agent would keep editing while a reconnect resumes the same session in a second process.
  - So the helper kills every process tree it started on stdin EOF or after missed pings, then exits.
  - It records its pids in `~/.sb-remote/<install-id>/pids`. At start it kills leftovers from an earlier helper that died hard.
  - `kill(channel, signal)` runs `killTree` (moved from acp.ts:117 into `lib.ts`) against the far side's own pids.
- **Liveness**: a ping every 15 s. Three missed pings mean offline: every channel of that host ends.
- **Cleanup**: old `<hash>` folders go only when unused (a lock file per running helper, last-used stamp older than 30 days), so one install never deletes a helper another one runs. ⚑

## How a session runs on a host

```mermaid
sequenceDiagram
  participant SB as Switchboard (laptop)
  participant R as sb-remote on desk
  participant A as claude-agent-acp on desk
  SB->>R: connect (bootstrap, install if needed), hello
  SB->>R: mcpUrl → 127.0.0.1:40213
  SB->>R: spawn [bun, …/claude-agent-acp], env = desk login env + SWITCHBOARD_MCP_SECRET
  R->>A: Bun.spawn
  SB-->>A: ACP initialize / session/new {cwd: /home/oskar/code/x, additionalDirectories (far-side paths), mcpServers:[http://127.0.0.1:40213/mcp/app]}
  A->>R: POST /mcp/app (post_message)
  R->>SB: frame → secret + host check → handleMcp → response frame
  A->>A: Bash: git worktree add … (on desk)
  A->>R: set_worktree /home/oskar/code/worktrees/x
  R->>SB: frame; SB stores it as a desk HostPath after host.fs.exists on desk
```

- ACP itself does not change. `acp.ts` already speaks ndjson over a process's stdio, and `host.spawn` returns a process with the same shape.
- Resume works, because the transcripts live in `~/.claude` on desk, where the adapter looks (session/resume, `sessionExists`).
- The contract, topic intro and hand-over header name far-side paths only ([Artifacts](#artifacts)).

## Artifacts

The agent writes its artifact folder with its own tools, so the folder has to exist on the far side. The app, the database and the operator read it on this machine.

**Phase 1: the far side is authoritative, and the local folder is a read-through cache** ⚑ ([Q2](#q2)).

- On the far side the folder is `~/.sb-remote/<install-id>/data/<todo-id>/`, and the contract names that path.
- The local `~/.switchboard/data/<todo-id>/` is a cache, refreshed from the far side:
  - on `attach_file`, for the file being attached, before the existing checks (apptools.ts:131);
  - at turn end, for the folder;
  - when the user opens a file of that TODO.
- Writes from the UI (upload, New note, editor save, Delete's move to `archive/`) go to the far side through `host.fs`, and then refresh the cache. With one writer per file there are no conflicts, no mtime comparisons across clocks, and no resurrected deletes.
- **Cache write rules** (this is the one place far-side data becomes local files):
  - The name must be relative, with no `..`, no backslash, no `:`, no control characters, no trailing dot or space, and no Windows device name (`CON`, `NUL`, `aux.txt`…).
  - Names that differ only in case are refused.
  - Only regular files are taken (lstat on the far side); symlinks and devices are skipped, and no link is ever created locally.
  - The parent's realpath must be inside the cache folder.
  - Each file is written to a temp file and renamed.
  - Caps: 25 MB per file, 500 MB and 5,000 files per TODO.
  - A name that breaks a rule is listed in the TODO's timeline as "not copied: <name> · <reason>".
- **Far-side paths in prompts and tool output.** `files.path` and attachment paths stay local, so anything shown to a remote agent goes through `host.toFar(local)`, a prefix swap from the local artifact root to the far-side one. The places that need it:
  - the hand-over header's file paths and `work/` folder (runner.ts:598-602);
  - uploads in the first prompt (`withAttachments`);
  - `read_todo`'s files and attachments;
  - `read_file`'s "open it with Read" path;
  - the router prompt's `Folder:` line;
  - the Claude terminal's `--add-dir`, `--mcp-config` and `--append-system-prompt-file`, and `SWITCHBOARD_FILES` (server.ts:941-960).

  A test lists every tool output and prompt builder and checks that a remote TODO's text contains no local path.

## Terminals (phase 2)

- **Shell**: `host.pty([loginShell, "-l"], {cwd: worktree})`. The bytes go from the helper's PTY over the pipe to the existing WebSocket (server.ts:1025). The `TERM_UNSUPPORTED` gate (server.ts:907) applies to the host that runs the PTY, so a Windows laptop gets terminals for remote topics before local ones work there.
- **Claude**: as today (server.ts:941-956). Its `mcp.json` and `contract.md` are written into the far-side artifact folder, with the relay's MCP URL.
- `termBusy` becomes an RPC to the far side's `ps`.

## UI

- **Settings › Hosts** (new section):
  - A list of hosts with a status dot (connected / offline · reason), OS and architecture, and topic count.
  - **Add host**: Name, SSH alias (hint "As you type it after `ssh`. Your ~/.ssh/config applies."), and **Test**.
  - Test shows a read-only checklist: ssh works, Bun ≥ 1.3.5, helper installed, git, gh (optional), Claude logged in. Each failed item gets a one-line instruction. Phase 1 has no fix buttons.
  - **Log in to Claude on desk** runs ACP `authenticate` through the host.
  - **Log** shows the helper's last 200 lines. **Remove** is refused while topics use the host.
- **Settings › Topics › topic**: a **Host** select (This machine / desk / …) above Folder. Folder is checked on that host, and the hint names it. The profile is written by hand for remote topics in phase 1; the profile scan on the host comes in phase 2.
- **TODO header and list rows**: a host chip `desk` on TODOs of remote topics. When the host is offline, the chip turns grey with "desk offline". Git state shows "—", and Start shows "desk is not reachable: <ssh error> · Retry".
- **Open in VS Code** on a remote topic opens `vscode://vscode-remote/ssh-remote+desk/home/oskar/code/x` (Remote-SSH) through the page. That also avoids `open -b` on macOS dropping arguments when VS Code already runs. Reveal is hidden.
- **Change topic** to a topic on another host is refused, with the reason.

These are UI changes, so each needs an entry in `UI-CHANGELOG.md`.

## Connection lifecycle

| Event | What happens |
|---|---|
| Server start | Hosts with live or scheduled TODOs connect in the background. Others connect on first need: opening a TODO or a file, Start, or the batched git state after a list. |
| Schedules ⚑ | `fireDue` (server.ts:1781) checks the host. When it is offline, the catch-up waits up to 60 s for the connect, then records a skipped run "desk offline" (`skipReason`). Routing does not pick a topic whose host is offline. |
| No channel open for 10 min | The pipe closes. The next need reconnects (about 1 s with the helper installed). |
| Pipe drops | The helper kills its processes. Running TODOs get "host disconnected" and the same status reset as "app restarted" (server.ts:1765). Reconnect uses backoff (1, 2, 5, 15, 30 s). The next message resumes the session on the far side. |
| Laptop sleeps | The same as a dropped pipe. |
| App update with a new helper | The new `<hash>` installs at the next connect. Live pipes keep the old helper until they close. |

## T-15: containers as a transport

T-15 designs a box per TODO, with the worktree on **this machine's disk** bind-mounted in. That choice causes most of its hardest parts:
- Windows path translation (`C:\…` ⇄ `/c/…`).
- Git worktree links that break on one side (its Q8).
- `.git` config and hooks as a way back to the host (hooks mounted read-only, hardened git, the config guard).
- Slow NTFS bind mounts.

With the host layer, a box is just another host: `ContainerHost`, transport `docker exec -i sb-t15 /opt/sb-remote/bootstrap`, with the helper baked into T-15's base image. That opens the VS Code Dev Containers option "**Clone repository in container volume**":

- The repo and its worktrees live **in a volume inside the box**. Git, the file browser, diff and terminals all run there through the helper, exactly as for an SSH host.
- **Gone**: path translation, the Q8 worktree-link problem, the `.git` escape (the host never runs git in the agent's repo, so the config guard and hardened git calls are not needed), and slow bind mounts on Windows.
- **Reused from this design**: the security rules above (host-scoped app tools, no far-side strings in local shells, the strict artifact cache) are exactly what a box needs too.
- **Cost**: your host editor and host tools do not see the files directly. VS Code's Dev Containers "attach" (`vscode://vscode-remote/attached-container+<hex>/<path>`) covers the editor, and Switchboard's file browser covers the rest. Pushing happens from the box (T-15 Q4 already plans for that).
- T-15's proxy, gateway, setup agent, recipes, caches and allow-lists stay as designed. Only "how Switchboard reaches the box" and "where the files live" change. The helper's MCP relay can also replace the gateway's `:4477` relay.
- Containers on a remote host (ssh, then docker) are a chain of two transports. That is later, but the interface allows it.

Recommendation ([Q6](#q6)): build the host layer first, then T-15 on top of it, and revisit T-15's "files on the host disk" choice.

## Phases

1. **Spike** (half a day, thrown away):
   - Check ACP over plain `ssh` with a custom agent row (boat-spikes Spike B, against desk instead of a VM).
   - Then a 100-line helper with the bootstrap, `spawn` and the MCP relay.
   - Measure: first turn time, tool-call round trip over Tailscale, a batched `git status` for 10 worktrees, and what desk's login shell prints.
2. **Phase 1a, refactor only**:
   - The `Host` interface plus `LocalHost`, with the `HostPath` brand.
   - Every call site in the table goes through `hostOf(topic)`, and `acp.ts` takes a spawner.
   - `killTree` moves into `lib.ts`; the git state cache is keyed by (host, path); grants carry a host.
   - The grep test from [The host interface](#the-host-interface).

   No behaviour change. This is worth having even if 1b stops.
3. **Phase 1b, SSH hosts**:
   - `remote.ts` plus `SshHost`: bootstrap, frames, exec, spawn, fs, git ops, MCP relay, process ownership.
   - Agents, git state, file browser, editor, diff, PR card and worktree scan on the host.
   - The artifact cache with its rules, and `toFar` everywhere.
   - Host-scoped app tools.
   - Settings › Hosts, the topic Host field, chips and offline states, schedules.
4. **Phase 2**: terminals on hosts, the profile scan on hosts, Test fix buttons (Install Bun), ports with forwards, a remote folder picker, WSL.
5. **Phase 3**: `ContainerHost` for T-15.

## Later

- **Agents that survive a dropped pipe.** The helper runs detached (`setsid`), and each ssh connection is only a client to it through a unix socket in `~/.sb-remote/`, which is how the VS Code Server survives reconnects. Agents keep working while the laptop sleeps. MCP calls made while no client is attached queue in the helper and replay on reconnect. That is a real feature with its own design: queued `post_message` order, permission requests nobody can answer, and a helper outliving the app on purpose.
- A sync of your Claude setup (plugins, skills, CLAUDE.md) to a host, together with T-15 Q5.
- Moving a TODO between hosts.

## Calls I made (check these)

- System `ssh` as the transport, not an embedded SSH library. It keeps your config, agent, keys and Tailscale SSH. The cost: no connection sharing on Windows (no ControlMaster), which is why there is one long-lived pipe per host and not one `ssh` per command.
- One helper for all transports, written in TypeScript with Bun, sharing `lib.ts`.
- Composite and batched operations (git state for all worktrees, tree, walk, containment check) run on the far side in one round trip; list views never wait on a host.
- MCP through a relay in the helper, not `ssh -R`.
- The far side is less trusted than this machine: host-scoped app tools, no far-side strings in local shells, a strict artifact cache.
- A host per topic, not per TODO; Change topic across hosts refused.
- No fallback to this machine when a host is offline.
- `gh` stays on this machine, fed with the repo name read through the host.

## Open questions

> **❓ Q1 — Bun on the far side: required, installed for you, or bundled?** · status: OPEN <a id="q1"></a>
>
> - **A. Required** (≥ 1.3.5); Test says how to install it.
> - **B. Installed on click**: **Install Bun** runs the official installer over the pipe.
> - **C. A compiled helper** (`bun build --compile`) per OS and architecture, about 60–90 MB each, uploaded at the first connect. No dependency, but the app ships four binaries and the first connect over a slow link is long.
>
> Recommendation: **A in phase 1, B in phase 2**. Your machines probably have Bun already.

> **❓ Q2 — Artifact folder: read-through cache, two-way mirror, or far side only?** · status: OPEN <a id="q2"></a>
>
> - **A. Read-through cache** (above): the far side is the only writer. The local copy refreshes on attach, at turn end and on open, and UI writes go to the far side. Everything that reads artifacts today keeps working.
> - **B. Two-way mirror**: both sides write and sync. Needs a manifest for deletes, conflict files and clock handling.
> - **C. Far side only**: one copy. Every artifact reader goes through `host.fs`, `files.path` gains a host column, and the operator cannot Read remote TODOs' files (only through `read_file`).
>
> Recommendation: **A**.

> **❓ Q3 — Host per topic or per TODO?** · status: OPEN <a id="q3"></a>
>
> - **A. Per topic**: "switchboard on desk" is its own topic beside "switchboard". Simple: a topic's folder, worktrees and transcripts are on one machine.
> - **B. Per TODO**: one topic, and you pick the machine when you start a TODO. Flexible (send a heavy TODO to desk), but every topic needs a folder per host, and `places`, routing and the profile scan become per host.
>
> Recommendation: **A**. B can come later as "Run on…" if A feels limiting.

> **❓ Q4 — When a host goes offline mid-turn, what then?** · status: OPEN <a id="q4"></a>
>
> - **A. The run ends** like an app restart. The helper kills its processes, and the next message resumes the session (phase 1).
> - **B. Persistent helper now** ([Later](#later)): agents keep running and catch up on reconnect.
>
> Recommendation: **A** in phase 1, B as its own design once phase 1 is in use.

> **❓ Q5 — Your Claude setup on the far side?** · status: OPEN <a id="q5"></a>
>
> - **A. Your business**: you install plugins and skills on each machine. Test lists which of your local plugins are missing there.
> - **B. Sync on click**, shared with T-15 Q5 (all but hooks, paths rewritten).
>
> Recommendation: **A** in phase 1, B together with T-15.

> **❓ Q6 — Order with T-15, and does T-15 move its files into the box?** · status: OPEN <a id="q6"></a>
>
> - **A. Host layer first, then T-15 as `ContainerHost`, with the repo in a box volume.** T-15 loses its Windows path and `.git` problems; host tools see the files only through VS Code attach.
> - **B. Host layer first, T-15 keeps its bind-mounted worktree** (its design as is, but reached through the helper).
> - **C. T-15 first as designed**, the host layer later.
>
> Recommendation: **A**.

## Open Questions

| ID | Question | Options | Recommendation | Status |
|---|---|---|---|---|
| Q1 | Bun on the far side | A / B / C | A now, B in phase 2 | OPEN |
| Q2 | Artifact folder | A / B / C | A (read-through cache) | OPEN |
| Q3 | Host per topic or per TODO | A / B | A | OPEN |
| Q4 | Offline mid-turn | A / B | A now, B later | OPEN |
| Q5 | Claude setup on the far side | A / B | A now, B with T-15 | OPEN |
| Q6 | Order with T-15 | A / B / C | A | OPEN |
