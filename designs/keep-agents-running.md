# Keep agents running when the app closes

Investigation, 2026-10-04, line numbers rechecked against main on 2026-10-07. No code changed.

## Short answer

Yes, if the Bun server (`bun server.ts`) keeps running after the window closes.
Agents and terminals cannot outlive the server, so the work is "who keeps the server alive", not "who keeps the agents alive".

## Why the server must stay up

Process tree of a server started from a terminal (observed with `ps` on 2026-10-04):

```
Terminal.app
└─ zsh (login shell)
   └─ bun server.ts
      ├─ claude --output-format stream-json   one per running agent (app run), same process group as the server
      └─ zsh -l / claude                      one per open terminal tab (spec item 111), on a pseudo-terminal the server holds
```

Each agent is a `claude` process that `query()` starts with three pipes to the server
(`stdio: ["pipe","pipe","pipe"]` in `spawnLocalProcess`, `node_modules/@anthropic-ai/claude-agent-sdk/sdk.mjs`, SDK 0.3.284).
Over those pipes the agent:

- calls the app tools (`post_message`, `set_title`, `set_attention`, ...). They run inside the server: `workerServer()` runner.ts:175, `operatorServer()` runner.ts:346.
- runs the hooks that record events and the last tool: runner.ts:271-284.

When the server's end of stdin closes, the agent exits: stdin EOF, ~2 s grace, then kill (sdk.d.ts:9396).
Even if the agent lived on, it could not post a message or raise attention.

Terminal tabs run under `Bun.spawn(cmd, { terminal })` (server.ts:470). The server holds the pseudo-terminal, so they end with it too.

## What stops the server today

| Trigger | Path |
|---|---|
| Red X (last window closed) | `window-all-closed` → `app.quit()` (electron/main.js:77), then the Cmd+Q chain below |
| Cmd+Q, or app quits | `will-quit` → `server.stdin.end()` (main.js:79) → server sees stdin end (server.ts:997) → `shutdown()` ends terminals (server.ts:989) and interrupts all agents |
| Ctrl-C in the terminal running `bun server.ts` | SIGINT → `shutdown()`, server.ts:994 |
| Logout / reboot | SIGTERM → `shutdown()`, server.ts:995 |
| Terminal tab or window closed (server started in a terminal) | SIGHUP. server.ts has no handler for it, so the server dies at once: no `shutdown()`, no interrupt. The `claude` agents share its process group and die too. Their TODOs stay `running` until the next server start resets them (server.ts:973). |

This is deliberate and written down in 2 places:

- spec item 56: "On shutdown (Ctrl-C, SIGTERM, or quitting the window app), the app interrupts all running agents, ends all terminals ..."
- README.md:11: "closing it stops running agents and terminals like Ctrl-C"

### Already works today: server started in a terminal

If a server already runs on the port, the copy Electron starts exits at once, and the window attaches to the live server
(comment at main.js:54, spec item 67). The window reads the live server's token from `~/.switchboard/token` (main.js:62).
Quitting Electron then only closes stdin of that dead copy (not tested whether that raises an error in Electron at quit).
The live server, its agents and its terminals keep running.

Checked on 2026-10-07: `env -u SWITCHBOARD_PARENT SWITCHBOARD_HOME=$TMPDIR/... PORT=4477 bun server.ts` while 4477 was taken
exits 1 with `code: "EADDRINUSE"` at server.ts:931. That happens before the token write (server.ts:967) and the
"app restarted" reset loop (server.ts:973), so the live TODOs and token stay untouched.

So: `bun server.ts` in a terminal + `bun run app` for the window = agents survive closing the window today.
Hole: they do not survive closing that terminal tab (SIGHUP row above). No-code fix: `nohup bun server.ts >> ~/.switchboard/server.log 2>&1 &`.
The work below is only needed if Electron itself should start the server.

Environment: `bun run app` from a terminal passes the terminal's environment to the server. A packaged app started
from the dock or Spotlight loads the login shell's environment first (`loginShellEnv()`, main.js:11-22). Both apply to every option below.

## Options

### 1. Red X closes the window, the app stays in the dock (macOS style)

```js
// electron/main.js
app.on("window-all-closed", () => { if (!mac) app.quit(); });
app.on("activate", () => { if (!BrowserWindow.getAllWindows().length) openWindow(); }); // dock click
```

- About 5 lines. Window creation moves out of `app.whenReady()` into an `openWindow()` function.
- Covers the red X only. Cmd+Q still stops all agents.

### 2. Electron starts the server detached; quitting Electron leaves it running

```js
// electron/main.js
const log = openSync(join(HOME, "server.log"), "a");
server = spawn("bun", ["server.ts"], { cwd, detached: true, stdio: ["ignore", log, log] });
server.unref();
// remove SWITCHBOARD_PARENT and the will-quit handler; server.ts:997 is then unused
```

- About 10 lines.
- `detached: true` puts the server in its own session and process group, so neither Ctrl-C on `bun run app` nor closing that terminal tab (SIGHUP) reaches it.
- Reopening the app attaches to the live server (EADDRINUSE path, checked above).
- Needs a way to stop or restart the server:
  - day one: `kill $(lsof -tiTCP:4477 -sTCP:LISTEN)`. SIGTERM already runs `shutdown()` and interrupts agents.
  - later: a button or menu item. That needs a new endpoint, for example `POST /api/shutdown`.

### 3. launchd LaunchAgent starts the server at login

- A plist in `~/Library/LaunchAgents`. `KeepAlive` restarts the server after a crash. Electron becomes only a window.
- Catch: launchd does not run shell startup files. `bun` is on PATH only through ~/.zshrc, and API keys live in ~/.zshenv.
  The server and every agent it starts would miss them.
  Fix: the plist starts `/bin/zsh -ilc "exec bun server.ts"`, the same login shell `loginShellEnv()` (main.js:11) already runs.
- Restart: `launchctl kickstart -k gui/$UID/<label>`. Stop: `launchctl bootout gui/$UID/<label>`. A plain `kill` does not stick: `KeepAlive` starts it again.
- Most moving parts. Worth it only for start-at-login or restart-after-crash.

### 4. Agents independent of the server: not recommended

Would need: app tools served over HTTP instead of in the server process, hooks as shell commands that call the server,
and a relay process that holds each agent's pipes so a restarted server can reattach.
Tool calls while the server is down still fail. Large rewrite; the only gain is surviving a server crash or code restart.

## What changes with option 1, 2 or 3

- Spec and README: item 56 drops "or quitting the window app", plus one line that closing the window does not stop the server. README.md:11 changes to match.
- Stale code: today, close and reopen restarts the server with new code. After the change, a `git pull` needs an explicit server restart.
- Unchanged: logout and reboot still stop the server via SIGTERM and interrupt agents (5 s limit, server.ts:990).
- Unchanged: after a server restart, running TODOs become `idle` with `app restarted`, and queued messages wait for Deliver (server.ts:973).

## Open question

What does "close the app" mean?

- a) Red X / closing the window → option 1 is enough.
- b) Cmd+Q too → option 2 (or 3 if start-at-login is wanted too).
- c) The server always starts in a terminal anyway → nothing to build. Start it with `nohup` so closing the terminal tab does not kill the agents.
