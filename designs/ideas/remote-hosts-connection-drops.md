# Remote hosts: surviving connection drops

Status: IDEA, for later · 2026-11-05 · not built. Line numbers are from main `8371dd7`.
An add-on to `remote-hosts.md` (its Q4 and [Later](remote-hosts.md#later)).

## The problem

The draft says: pipe drops, so the helper kills the agent, and the next message resumes the session. That is safe, but on a laptop it means:

- Wi-Fi switch, VPN blip, lid closed for lunch: every remote turn dies.
- A turn killed mid-tool can leave half-done work (`.git/index.lock`, a half-applied edit, a test run cut off), and you have to notice it and say "continue".
- A long autonomous turn (an hour of refactoring) is only as reliable as your Wi-Fi.

You are right that this makes remote topics unusable for real work. Drop tolerance has to be in phase 1, not "later".

## First: the browser setup has no drop problem

With `../browser-on-lan.md` (Switchboard runs on desk, the laptop only opens a browser), the agents and the server are on the same machine. A drop only breaks the page's event stream, and `EventSource` (index.html:4993) reconnects by itself. Nothing is lost, and nothing has to be built.

So the question is only for remote hosts, where **one** Switchboard on the laptop drives agents on other machines. If one Switchboard per machine and a browser tab per machine is enough for you, the browser setup is the cheaper answer to drops. The rest of this file assumes you still want remote hosts.

## What drops there are

| Drop | How long | What breaks today in the draft |
|---|---|---|
| Wi-Fi blip, AP roam, Tailscale re-handshake | 1–30 s | Often nothing: Tailscale keeps the IP, so the TCP connection of `ssh` can survive. Our own ping (3 × 15 s) may still call it offline. |
| Network change (home to phone hotspot) | 10 s – 2 min | `ssh` usually dies: ServerAlive gives up, or the old route is gone. |
| Laptop sleep | minutes to hours | `ssh` dies on wake. Timers jump, so the ping check fires at once. |
| Switchboard restart on the laptop (update, crash) | seconds | Every in-memory ACP connection is gone. |
| Desk reboots | minutes | The agents are gone. Nothing can save them; resume is the answer. |

## Three levels

### Level 0: cheap fixes (hours)

1. **Tolerant liveness.** `ssh -o ServerAliveInterval=15 -o ServerAliveCountMax=8` (2 min), and the app ping counts misses on a monotonic clock and ignores a gap right after a wake (a wall-clock jump bigger than the ping interval). Short blips then never reach the app.
2. **Auto-continue after a cut.** When a run was ended by a drop (not by you), the TODO gets "desk dropped mid-turn". At reconnect, the app resumes the session and sends: "The connection dropped at 14:02 and your turn was cut, possibly during a tool call. Check the state (for example `git status`, lock files) and continue." A topic setting turns it off.

That helps, but a cut is still a cut: the tool call in flight is lost, and the agent spends a turn re-orienting.

### Level 1: the agent keeps running (the VS Code Server model) — recommended

This is how VS Code Remote-SSH survives drops: the server on the far side does not belong to the ssh connection. A reconnect attaches to it again, and both sides replay what the other missed. (VS Code keeps a disconnected server's state for hours.)

**1. A helper that outlives the pipe.**
- The bootstrap starts the helper detached (`setsid`, stdio to a log file). It listens on a unix socket `~/.sb-remote/<install-id>/sock` (mode 0600, in a 0700 folder).
- The ssh connection only runs a small `attach` mode of the same script: it copies bytes between the pipe and the socket. A drop kills the `attach`, not the helper.
- At connect, the bootstrap attaches to a running helper, or starts one.

**2. Sequence numbers and replay.**
- Every frame in either direction gets a sequence number; each side acks what it has received (piggybacked on pings, at least every few seconds).
- Each side keeps unacked frames. On reattach: `resume {lastSeen}` both ways, then each side resends from there.
- The helper keeps its outbound frames in an **append-only journal on disk** per channel (`journal/<channel>`), trimmed on ack. A long drop with a chatty agent does not fill memory, and a crash of the helper does not lose them. Cap: 200 MB per host. Past the cap, the helper stops reading the agent's stdout (backpressure: the agent waits, it does not lose data).
- The laptop keeps its few outbound frames (prompts, answers, keystrokes) in memory.

**3. The laptop pauses instead of ending.**
- `SshHost` has a fourth state, **reconnecting**: channels stay open, their streams just stop delivering. `host.spawn` still returns the same `Proc`; its `stdout` waits.
- So `acp.ts` does not notice the drop. The pending `session/prompt` request stays pending and gets its answer after the replay. **ACP and the runner do not change.** This is the main reason to do it at the transport layer.
- What does change in `acp.ts`: its "stopped answering" check (acp.ts:152) and `withTimeout` on requests must not fire while the host is reconnecting.

**4. What the agent meets while nobody is attached.**

| The agent does | While detached |
|---|---|
| Streams text, tool calls, plans (`session/update`) | Journaled, replayed on reattach. The timeline catches up at once. |
| Asks permission (`requestPermission`) | Waits, as when you are away from the desk. On reattach the request reaches the app and raises attention as usual. |
| Calls a write-only app tool (`post_message`, `set_title`, `set_attention`, `suggest_follow_ups`, `set_effort`, `set_worktree`, `set_pr`, `attach_file`) | The helper answers "ok" at once and queues the call. On reattach the calls replay in order. A call that fails then (an `attach_file` of a file that is gone) becomes a timeline event. |
| Calls a reading or creating tool (`read_todo`, `read_file`, `list_todos`, `list_comments`, `create_todo`…) | Answered at once with "Switchboard is not connected right now; try again later or carry on without it." No hanging MCP request, so no MCP timeout in the agent. |
| Runs Bash, edits files, runs git | Nothing changes. That is the point. |

Which tool is which is a list in the helper, next to the tool definitions (apptools.ts), with a test that every tool is in exactly one group.

**5. What you do while it reconnects.**
- The TODO shows **desk reconnecting · agent still running** (not "offline"), and the run stays *running*.
- A message you send is queued, as messages sent during a run are today. **Stop** is queued too, and is the first frame after reattach.
- Opening files, the diff or git state of that host shows "—" until it is back.

**6. How long the helper waits.**
- A detached helper keeps its agents for a **grace time**, default 12 h (a night's sleep), set per host.
- A turn that ends while detached just ends; the agent process idles, as it does locally.
- Past the grace time the helper ends its agents and exits. On reattach after that, the app sees "channel gone" and falls back to Level 0 (resume and auto-continue).
- An idle helper with no agents and no client exits after 10 min.

**7. The rules that keep it safe.**
- **Never two processes for one session.** At reattach the helper lists its live channels. The app resumes a session (`session/resume`) only when its channel is gone. The draft's reason to kill on EOF (a second process editing next to the first) is solved by reattaching instead.
- **The pids file** stays: a helper that starts and finds leftovers from a dead helper kills them, but only when no live helper holds the socket (a lock file).
- **Helper upgrades with live agents.** A new app version brings a new helper hash. The old helper keeps running until its agents end; the bootstrap attaches to it for those channels and starts the new one for new channels (two sockets, two `attach`s over one ssh, or two ssh pipes). Simplest first version: new channels wait for no-agents, then the old helper is replaced. To decide when it is built.
- **The socket is the only way in**, and only for the same user. Same trust as ssh itself.

**8. Terminals get it for free.** A PTY is just another channel: its output is journaled (capped at the last 1 MB, like a scroll-back) and replays on reattach, like `tmux attach`.

### Level 2: survive a Switchboard restart on the laptop (later)

Level 1 keeps the agent alive, but the laptop's `acp.ts` connection objects are in memory. After an app restart, the pending request ids are gone. To adopt a running remote agent, the helper would track each session's state (turn in progress, pending permission, the prompt request id) and the app would rebuild a run from it.

Until then: the restart reset (server.ts:1784-1791) ends remote runs like local ones, the helper is told to cancel them (`session/cancel`, not a kill), and auto-continue picks them up. Restarts are rare compared to sleep and Wi-Fi, so this can wait.

## What it costs

| Part | Where | Size (rough) |
|---|---|---|
| Tolerant ping, ServerAlive, wake detection | SshHost | small |
| Auto-continue after a cut | runner, a topic setting | small |
| Detached helper, socket, `attach` mode, lock file | remote.ts, bootstrap | medium |
| seq/ack, resume handshake, disk journal, backpressure | the frame layer on both sides | medium; the hardest part, and the best tested |
| `reconnecting` state, paused streams, timeouts that wait | SshHost, acp.ts:152 and withTimeout | small |
| MCP calls while detached: queue or refuse, replay | helper relay, apptools.ts list | small–medium |
| Grace time, cleanup, upgrade rule | helper | small |
| UI: "reconnecting · agent still running", host grace time | TODO chip, Settings › Hosts | small, plus `UI-CHANGELOG.md` entries |

Level 1 is roughly a third more work than phase 1b as drafted. Most of it is the frame layer, which can be tested hard without ssh: a fake pipe that drops, delays and reorders at random, and a check that both sides end up with every frame exactly once, in order.

## What changes in the remote-hosts design

- **Q4 → B**: the persistent helper is phase 1, not later. Non-goal 1 goes.
- **Framing**: sequence numbers and acks from the first version (even before the helper is detached), so Level 1 is not a protocol change.
- **Process ownership**: "kill on EOF" becomes "kill after the grace time, or when the app says Stop".
- **Liveness**: offline after the grace time; before that the host is *reconnecting*.
- **Phases**: 1b ships Level 0 for testing on a steady LAN. 1c adds Level 1. Remote topics count as usable only after 1c.
- **Spike**: add one measurement: does a Tailscale ssh connection survive a Wi-Fi switch and 2 min of sleep with ServerAliveCountMax=8? That tells how much Level 0 alone gives.

## Open questions

> **❓ D1 — How far, in phase 1?** · status: OPEN
>
> - **A. Level 0 only** (tolerant ping, auto-continue). Cheap; a drop still cuts the turn.
> - **B. Level 0 + Level 1** (agent keeps running, replay). A third more work; drops and sleep are invisible.
> - **C. Skip remote hosts, use the browser setup** for machines you want to work on remotely. No drop problem at all, no new code for it; one tab per machine instead of one app.
>
> Recommendation: **B** if one app over several machines is the goal; **C** if a tab per machine is fine for you.

> **❓ D2 — Grace time default?** · status: OPEN
>
> - **A. 12 h** (a night). An agent may work unseen for that long.
> - **B. 1 h**. Covers meetings and short sleeps; a night ends the run.
> - **C. Until the turn ends, plus 1 h**. The agent finishes what it was doing, then waits an hour for you.
>
> Recommendation: **C**. A turn that is running finishes; nothing is left running for hours with nobody around.

> **❓ D3 — Auto-continue after a cut: on or off by default?** · status: OPEN
>
> - **A. On**: the agent picks up by itself, with a prompt telling it to check for half-done work.
> - **B. Off**: the TODO says "cut by a drop · Continue", one click.
>
> Recommendation: **B**. With Level 1 a cut only happens after a long absence or a desk reboot, and then you want to look first.
