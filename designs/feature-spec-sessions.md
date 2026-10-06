# Switchboard: sessions in a TODO

The next phase of Switchboard. Today each TODO is exactly one Claude Code session (item 9). In this phase a TODO can hold several sessions, one after another. Work on one ticket (research, then development, then review fixes) can start each step with a fresh context and keep its worktree, PR, files and card. A TODO with one session, such as a PR review, works exactly as it does today.

Item numbers continue from `feature-spec-switchboard.md`, the main spec. "Item N" means item N of either file; the main spec's items end at 67. When this phase is built, its items and the changes listed below are merged into the main spec.

## Why

Today, continuing a ticket with a fresh context means making a new TODO. You have to tell it the worktree path. It cannot register the old TODO's files, because `attach_file` accepts only files in its own folder (item 23). One ticket ends up as several cards that are removed one by one. Only the operator has a New session button (item 38), and it hides the older messages instead of keeping them.

## Terms

```
Project              one repository, local folder, agent prompt (unchanged)
└─ TODO              title, worktree, PR, artifact folder, status, attention, chat
   ├─ Session 1      ended: history
   ├─ Session 2      ended: history
   └─ Session 3      live: receives messages, runs, is resumed
```

- **TODO**: the unit of work the user adds, starts, steers and removes, often one ticket. The name stays.
- **Session**: one Claude Code session that Switchboard launched for a TODO. Sessions are numbered from 1, and only the latest one is live.
- A `claude` the user starts in a terminal or any other way is not a session of a TODO (item 83).

## Design

The sketches below define what the screens show and how they behave. Before this phase is built, the design gets a new version with the session timeline, the New session composer and the session count on the card. From then on the UI follows the design, as the main spec requires. The sketches use the components of design v3.

Overview card, with a session count once there is more than one session:

```
┌────────────────────────────────────────────┐
│ ● Idle                              sb-api │
│ Implement MCPs (AI-1234)                   │
│ ⎇ ai-1234-mcp · PR #512 ✓ · session 3      │
│ agent: Pushed, PR #512 updated.            │
└────────────────────────────────────────────┘
```

Focus view, where the Chat tab is a timeline of the sessions:

```
┌──────────────────────────────────────────────────────────────────────────┐
│ Project sb-api   Session 3 of 3 · 7f2c…   Branch ai-1234-mcp             │
│ 4 commits · 2 changed · pushed       PR #512 · checks ✓                  │
│ Implement MCPs (AI-1234)                [Idle] [New session] [Remove]    │
│ Chat 23   Files 4   Events                                               │
├──────────────────────────────────────────────────────────────────────────┤
│ ▸ Session 1 · Research MCP options for AI-1234 · Oct 3 09:12–10:40       │
│   agent: Options compared in plan.md, recommend B.         plan.md       │
│ ▸ Session 2 · Implement the server side · Oct 3 11:02–16:30              │
│   agent: Server done, client stubs left.             impl-notes.md       │
│ ▾ Session 3 · Finish the client stubs and add tests · Oct 4 08:50–       │
│   you    Finish the client stubs and add tests                           │
│   agent  Client done, tests green. Push?                                 │
│   you    push                                                            │
│   agent  Pushed, PR #512 updated.                                        │
├──────────────────────────────────────────────────────────────────────────┤
│ [ message session 3 …                                        ] [Attach]  │
│ Delivers now (idle)                                                      │
└──────────────────────────────────────────────────────────────────────────┘
```

The composer after New session is clicked:

```
├──────────────────────────────────────────────────────────────────────────┤
│ New session 4                                                 [Cancel]   │
│ [ first message for session 4 …                              ] [Attach]  │
│ ⌘↵ starts session 4. Session 3 ends and stays above as history.          │
│ Handed over: worktree, PR #512, plan.md, impl-notes.md                   │
└──────────────────────────────────────────────────────────────────────────┘
```

## Functionality

### Sessions

68. A TODO holds one or more sessions, numbered from 1. Only the latest session is live: committed messages, runs, status and attention all concern it. Earlier sessions are history, and the app sends them no more messages or runs.
69. The title, worktree, PR, artifact folder, files, status, attention, draft and Unread state belong to the TODO, so all its sessions share them. A worktree or PR registered in one session applies to every later session.
70. Session 1 starts the way a TODO starts today: Start on a `not started yet` TODO (item 8).
71. Every later session starts with New session in the Focus view.
    - The button shows when the TODO is `idle` or `waiting on you`, and is disabled while it is `running`.
    - Clicking it switches the composer to new-session mode (see the sketch). The composer then shows a "New session n" heading with Cancel, and a hint saying that ⌘↵ starts session n and that session n−1 ends and stays as history. It also lists what is handed over (item 73).
    - The draft and any attachments already picked stay in the composer. ⌘↵ with text or attachments starts the session at once.
    - Cancel or Esc returns to the normal composer and keeps the draft. The mode is kept only in the browser: after a reload the composer is in normal mode, with the draft kept.
72. Starting session n (n > 1) does the following:
    - It sets `ended_at` on session n−1 and generates a new session id, as in item 9.
    - If the attention flag is raised, it clears it with actor `user`.
    - It records a `new session` event that holds the hand-over header.
    - It sends the hand-over header, every queued message and the new instruction as one prompt. Queued messages go to the new session because the ended session gets no more runs.
73. The app builds the hand-over header from the TODO's data; no agent writes it:
    ```
    This TODO continues earlier work (session 3). Title: Implement MCPs (AI-1234). Keep this title.
    Your worktree: /Users/me/code/worktrees/ai-1234-mcp. Work there.
    Pull request: #512.
    Last message of session 2: Server done, client stubs left.
    Files from earlier sessions (open with Read what you need):
    /Users/me/.switchboard/data/sb-api/41/plan.md (session 1)
    /Users/me/.switchboard/data/sb-api/41/impl-notes.md (session 2)
    ```
    - Lines without a value are left out.
    - The file list is every file registered in the TODO, each with the session that registered it.
    - Later turns of the session get only the worktree line, as today (item 46).
74. The app does not write a "where I left off" note. If the user wants one, they ask the live session for it before starting the next. The note is a file like any other, so it appears in the next header.
75. A removed TODO keeps its sessions. Restoring it resumes the live session (item 14), and the checks of item 15 use that session's id. Removing a TODO does not end its live session; only New session ends a session.

### Focus view and overview

76. The Chat tab shows the TODO's sessions as a timeline, oldest first.
    - Each session has a header line with its number, its label, its start and end time, and its session id with a copy button. The label is the first line of the session's first user message.
    - The live session is expanded, showing its chat with the composer below.
    - Earlier sessions are collapsed to their header, their last agent message and the files registered in them. Expanding one shows its chat read-only.
    - A TODO with only one session shows no session header, so it looks the same as today.
77. A user message belongs to the session it was delivered to. An agent or app message belongs to the session it was posted in. Pending and queued messages show at the end of the live session.
78. Each earlier session shows its session id, so the user can resume it outside Switchboard by running `claude --resume <id>` from the project's local folder. That folder matters because Claude Code stores transcripts under the folder a session was launched from (item 44). Switchboard does not track such a resume.
79. The Focus view's top row shows `Session n of n · <id>`. The Files tab shows, for each file, the session that registered it. The Events tab shows each event's session number.
80. A card shows `session n` on its git and PR line when the TODO has more than one session. Lanes, sorting, attention, Unread and the tab title do not change (items 40, 41, 55).

### Operator

81. The operator's tools `list_sessions` and `read_session` are renamed `list_todos` and `read_todo`, because "session" now names a part of a TODO. `read_todo` returns the TODO's sessions (number, id, label, start and end time) and its messages grouped by session. The operator still never starts a TODO or a session (item 35).

### Terminals

Terminals are designed separately. These items define how terminals relate to sessions, and they are built together with terminals.

82. A terminal belongs to a TODO. It opens in the TODO's worktree, or in the project's local folder when no worktree is registered.
83. Whatever runs in a terminal, including `claude`, is the user's own process, like a Claude Code session started in another terminal app today. It is not a session of the TODO:
    - it does not change the TODO's status, attention or chat;
    - it does not appear in the timeline;
    - the one-live-session rule of item 68 does not limit it.

    This makes a terminal the untracked way to work in parallel inside one TODO. A second TODO is the tracked way.
84. A terminal's environment includes `SWITCHBOARD_TODO_ID` and `SWITCHBOARD_FILES` (the TODO's artifact folder), so a `claude` started there can be told to read or write the TODO's files.
85. The Files tab lists every file in the TODO's artifact folder except `uploads/`, not only the files registered with `attach_file`, so files written from a terminal show up too. A file that no session registered is marked "added outside a session".

### Changes to existing items

| Item | Change |
|---|---|
| 5 | The card shows the session count (item 80). |
| 8 | Start launches session 1. |
| 9 | A started TODO has one or more sessions (item 68). The app generates each session's id and passes it on that session's first turn. The TODO stores the id of its live session. |
| 11 | `not started yet` exists only before session 1. New session does not return a TODO to it. |
| 13 | The Removed view shows the live session's id and the number of sessions. |
| 14, 15 | Restore resumes and checks the live session (item 75). |
| 23 | The Files tab shows the session that registered each file (item 79). With terminals, it lists the whole folder (item 85). |
| 31 | In the contract, the first line says the session is part of a TODO that can span several sessions. `set_title` is called on the first turn of session 1 only. New rule: when the prompt starts with the hand-over header, keep the title, work in the registered worktree instead of creating a new one, and read whichever listed files are needed before starting. |
| 34 | The tools are renamed (item 81). |
| 42 | The database also stores sessions. |
| 58 | The Chat tab of the Focus view is the session timeline (item 76). |

## How it is built

- **Data.** A new table `sessions (id, todo_id, no, session_id, started_at, ended_at)`, unique on `(todo_id, no)`. `messages`, `events` and `files` get a `session_no` column. A message's `session_no` is set when it is delivered (`take()` in `runner.ts`) or, for agent and app messages, when it is written. `todos.session_id` stays and always holds the live session's id, so delivery (item 20) and restore keep resuming `todos.session_id` without changes.
- **Migration.** This follows the existing `pr_number` migration in `server.ts`. On startup, every TODO that has a `session_id` but no `sessions` row gets session 1, with `started_at` taken from its first `launch` event. Its delivered messages, its events and its files get `session_no = 1`.
- **Starting.** `start()` in `runner.ts` becomes the one place that opens a session: it inserts the row, sets `todos.session_id` and runs with `{ sessionId }`.
  - Start uses it for session 1, with the pending messages.
  - `POST /api/todos/:id/new-session` uses it for every later session, with the queued messages plus the new one. Its body and attachments are the same as for a message. It returns 409 while the TODO runs, before session 1 and after removal.
- **Prompt.** The hand-over header is a pure function in `lib.ts`, covered by `bun test`. `deliveryPrompt` uses it on the first turn of every session after the first.
- **Contract.** It changes as described for item 31, and is versioned with the app as before.
- **Files listing (item 85).** The Files fetch also registers files found in the artifact folder, with no `session_no`, so they get a file id the editor can open.

## Not built on purpose

- Parallel app sessions in one TODO. Use a second TODO, or a terminal (item 83).
- Messaging, resuming or restarting an earlier session from the app. Item 78 covers resuming one outside the app.
- Removing a single session, or moving sessions between TODOs.
- A task, tag or any other grouping level above the TODO.
- Hand-over notes or summaries written automatically by an agent (item 74).
- Starting a new session automatically, for example when the context fills up.
- Tracking `claude` processes that run in terminals. That would need the app's hooks and MCP tools inside a separate CLI process, which means the HTTP hook and MCP endpoints the main spec avoids. If it is ever wanted, a `kind` column on `sessions` is where it would go.

## Open questions

- Does the agent of a later session read the handed-over files on its own, or only when the instruction says so? Check this in a spike, like the main spec's spike findings, before relying on the header alone.
- Does `claude --resume <id>`, run in a terminal from the project's local folder, pick up a session that the Agent SDK created? Expected yes, because both use the same transcript store, but this has not been verified.
