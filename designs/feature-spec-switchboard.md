# Switchboard

Switchboard is a local app, in the browser or in its own desktop window, where every TODO runs as one or more Claude Code sessions, one after another. The user captures TODOs across multiple projects in a backlog, starts each one when ready, steers it with short one-line chat messages, and stops or restores it by removing or restoring the TODO. Anything substantial an agent produces (plans, reviews, questions with context, designs) goes into files the user can open and edit inline, so the chat stays small. Each TODO also opens its worktree's files and terminals of the user's own. A pinned operator session answers questions about all TODOs and acts only when told. A PR review is a TODO like any other. Guiding principle: **streamline the user's work, don't delegate their control.**

**This is an organizer, not an auto-builder.** Worker agents run exactly as the user's normal Claude Code sessions do: same user, project and local settings, same global `CLAUDE.md`, skills, plugins, hooks and MCP servers. The app adds a way to see, start, steer and stop them. It does not add restrictions, tool allowlists or safety layers beyond what the user's normal sessions have.

This document describes Switchboard's functionality and how it is built, and is the guideline for building it. Each numbered item is one piece of functionality. The numbers are stable, because the designs and code comments cite them (for example `R25` in `index.html`). Items are grouped by area, not by number; a new item takes the next free number.

## Design

The UI follows the design `designs/Switchboard-v4.html`. `designs/Switchboard-v1.html` to `designs/Switchboard-v3.html` are earlier versions, kept for reference.

The design decides layout, labels and placement. This spec decides behaviour, and its items already follow the design wherever the two used to differ. Where the design is silent, or where an accepted default overrules it (closing Settings returns to the view it came from, terminal numbers are never reused, Diff stays available while a file is locked), the items here decide. Where the UI and the design differ, the UI is changed to match the design, with these exceptions:

- Item 60: the design's "Show flow" panel and launch command preview are not built.
- The design's "Open questions" switches keep today's defaults: lanes by status, Unread does not count as waiting, a merged PR shows as a violet chip, and "behind main" is muted. Their other options are not built.
- The design draws window controls in HTML. The window app uses the native ones on macOS and keeps the native frame on Windows and Linux (item 121).
- The design draws the markdown editor with bytemd. The app uses CodeMirror 6 for every file, styled like the design (items 52 and 107).
- Where the design leaves out something the app does, the app keeps it:
  1. A card shows the PR line for a PR registered with `set_pr`, also without a worktree (item 5).
  2. An `app` message wins over the last agent message as a card's last message (item 5).
  3. `app` messages are labelled "App", in amber.
  4. A message being sent shows "Sending" while the agent runs, otherwise "Resuming".
  5. Operator replies are rendered as markdown (item 54).
  6. The Deliver bar: "N queued messages not delivered yet." with Deliver (item 56).
  7. Chat empty states, such as "No messages yet."
  8. The TODO files list shows "1 line" and "missing on disk", with Open disabled for a missing file (item 23).
  9. Unsaved file edits are kept when the file closes, and the file reopens in Edit (item 25).
  10. ⌘E and ⌘S work only while the file is visible (item 59).
  11. The new-project flow: "New project · not saved yet", then Create project (item 1).
  12. The Removed list is newest first, with singular and plural ("1 TODO", "2 TODOs") (item 13).
  13. Dialogs: Esc closes them, other keys are blocked, and focus stays inside.
  14. The restore warning text comes from the server (item 15).
  15. Each view keeps its scroll position.
  16. Clicking the `Board` or `Backlog` tab from the Focus view returns to that side's overview.

UI changes are checked against the running app on localhost, in Chrome or in the window app.

## Functionality

### Running the app

64. `bun server.ts` serves the app on `http://127.0.0.1:4477`. `PORT` changes the port and `SWITCHBOARD_HOME` the data folder (default `~/.switchboard`). At start it prints the link with the launch token, `http://127.0.0.1:4477/#token=…` (item 120). Opening that link is how a browser gets in.
65. `bun run app` opens the app in an Electron window (macOS, Windows, Linux) that starts `bun server.ts`. When the window app quits or crashes, the server's stdin closes and the server shuts down as in item 56. Links in rendered markdown open in the default browser. If a server already runs on the port, the window attaches to it. Once the server answers, the window reads the launch token from `$SWITCHBOARD_HOME/token` and loads the app with it (item 120), also when it attached to a running server, so the user never handles the token. The window's title bar, menu and icons are item 121.
66. `bun run install-local` builds `Switchboard.app` (no installer) and copies it to `/Applications`, so it opens from Spotlight and stays in the Dock. It quits a running Switchboard first, which stops its agents. The installed app runs a copy of the code (run `install-local` again to update it) and still needs `bun`. It loads the login shell's environment (`zsh -ilc`), because apps started from the Dock get a bare environment, so `bun`, hooks and MCP servers find their PATH and keys.
67. A second copy started on a port that is in use exits without touching any TODO: the startup reset of item 56 runs only after the port is bound. It also leaves the running server's token file alone, because the token file is written only after the port is bound (item 120).
126. The app works offline. `index.html` loads no script, style or font from the network: the fonts (Chakra Petch, Space Grotesk, JetBrains Mono), marked, DOMPurify, the code editor and the terminal are bundled in `vendor/` and served by the app's own server. With no network and an empty browser cache, the app loads, renders markdown and shows its own fonts.

### Projects

1. Settings › Projects (item 122) creates and edits projects, each with a name, a local folder path (`~` is expanded) and a free-text agent prompt. The repository is not typed in: the app reads it with `git -C <local_path> remote get-url origin` and shows it read-only. Saving a project whose local folder is not a git repository fails with an error. The section lists each project with its repository and TODO count. A new project shows as "New project · not saved yet" until the user saves it with Create project.
2. A project maps to exactly one repository.
3. Agents for a project launch with the project's local folder as their working directory and with all setting sources loaded (user, project, local), so the user's global `CLAUDE.md`, skills, plugins, hooks and MCP servers apply, as well as the repo's `CLAUDE.md`, `CLAUDE.local.md` and `.claude/` configuration.
4. The project's agent prompt is delivered to every agent launched for that project, including resumes and new sessions, appended to the Claude Code system prompt after the app contract (item 31). Each new agent process gets the current contract and project prompt, and an edited one replaces the process at the next turn, so a resumed agent sees the current versions, not the ones from its first turn (see prompt layering under How it is built). Typical content: instructions to use the user's worktree skill, which also symlinks untracked local files (e.g. `CLAUDE.local.md`, `.env`) into new worktrees.
43. Each project gets a slug when it is created. The slug never changes, even when the project is renamed, because the project's artifact folder is named after it (item 22).
44. A project's local folder becomes read-only once any TODO in that project has started, because Claude Code stores session transcripts under the folder the session was launched from, and a changed path would make every started TODO impossible to resume.
45. Projects cannot be deleted.

### TODOs and sessions

5. TODOs that have started show as cards on the board (item 87); TODOs that have not started show as rows in the backlog (item 88). A card shows the status with Queued, Draft and Unread badges, the title, the project, and the branch when a worktree is registered (item 46). The branch is read with `git --no-optional-locks status --porcelain=v2 --branch` in the worktree. The card also shows the last message (an `app` message wins over the last agent message), the time of that message, and a footer with the last tool, the messages waiting for the start, or the file names.
   - A TODO with a worktree shows a git line: commits on the branch, changed and untracked files, push state, and the session number (item 80). The Focus view also shows how far the branch is behind its base (item 124). Git state is read on every list fetch.
   - A TODO with a worktree, or with a PR registered via `set_pr`, shows a PR line that links to GitHub: state, draft, conflicts, checks, review decision, open threads, behind base. PR state comes from one `gh api graphql` call per project every 2 minutes and after each run, run as whichever logged-in `gh` account can read the repository, without switching the active account.
   - When `gh` fails, the last PR result stays, greyed. The Focus view shows the reason with a Retry now button (item 124), and the title bar shows a PR status chip (item 121).
80. With 2 or more sessions, a card's git line ends in `· session n`, the live session's number. A TODO without a worktree has no git line, so its card shows no session number. Lanes, sorting, attention, Unread and the tab title do not change with sessions (items 40, 41, 55).
6. TODOs are added in the backlog view, in the New TODO card (item 86). Adding takes a project, a first instruction and optional attachments (item 62). The project defaults to the one picked in the project filter (item 63), else to the most recently chosen one. Add (⌘↵) creates the TODO as `not started yet` at the top of the backlog, and no agent is launched. The card's note reads "Lands in the backlog. Nothing launches until you start it." The toast reads "Added to the backlog" / "<first 48 characters>. Start it when you have room."; when the project filter hides the new TODO's project, the toast says so instead. Add and Add & start are disabled while the text is blank, no project is picked, or a request is running.
92. Add & start (⌘⇧↵) creates the TODO and starts it in one request, so it skips the backlog and appears on the board as `running`. It records the same events as the two separate actions (`created`, then the start's run events). The user stays in the backlog and gets the toast "Started" / "<first 48 characters>. It is on the board now." with an Open action (item 125). When the project filter hides the project, the toast ends "…It is on the board, hidden by the project filter."
7. A TODO's initial title is its first instruction. Once the agent runs, it replaces the title with a generated one via `set_title`, in session 1.
94. While a TODO has not started, its Focus view lets the user edit the first instruction and change the project. Both controls are gone once the TODO starts.
    - The first message has an Edit action (tooltip "Edit the first instruction · until you start") that turns it into a textarea with the hint "Also updates the title. Attachments stay; add more as notes." ⌘↵ saves, Esc cancels.
    - The project in the Focus info line becomes a select (tooltip "Project · editable until you start"), and the line adds the note "not started · instruction and project editable until you start".
95. Saving a new first instruction updates that message's body. The title follows only while it still equals the first instruction (item 7). The save counts as a touch and moves the TODO to the top of the backlog. It records an `edited` event with the detail "first instruction" and shows the toast "Saved" / "First instruction and title updated. It moved to the top of the backlog."
96. Changing the project is refused (409) while the TODO has a running terminal, because its folders point into the old project. Otherwise it moves the TODO's artifact folder from `data/<old-slug>/<id>/` to `data/<new-slug>/<id>/` and rewrites the stored attachment paths, so the first instruction's attachments go with it. Nothing else is stored per project before the start: no session, worktree or registered files exist yet. It records an `edited` event with the detail "project A → B", "Moved …/A/<id>/ → …/B/<id>/" and "Attachment paths rewritten.", and shows the toast "Moved to <name>" / "Artifact folder moved to data/<slug>/<id>/ with its attachments."
97. The server refuses an edit once the TODO has started (409), for example after a start from another browser tab. The textarea then keeps the unsaved text and shows "409 · This TODO has started, maybe from another tab. Editing is closed; your text is kept here so you can copy it."
98. Attachments of the first instruction are not edited. More files and context go in as notes: messages that wait for the start (item 20).
8. The user starts each TODO explicitly, with Start on its backlog row or in the Focus view, or with Add & start (item 92). Starting launches session 1 with the first instruction and every message committed before the start, as one prompt. The TODO then moves from the backlog to the board. A start from a backlog row leaves the user in the backlog (item 89); a start from the Focus view stays there.
9. A started TODO has one or more Claude Code sessions, one after another (item 68). The app generates each session's id and passes it on that session's first turn, so the id is known before the agent runs. The TODO stores the id of its live session.
68. A TODO holds one or more sessions, numbered from 1, so work on one ticket (research, then development, then review fixes) can start each step with a fresh context and keep its worktree, PR, files and card. Only the latest session is live: committed messages, runs, status and attention all concern it. Earlier sessions are history, and the app sends them no more messages or runs. A `claude` the user starts in a terminal, or any other way, is not a session of the TODO (item 83).
69. The title, worktree, PR, artifact folder, files, status, attention, draft and Unread state belong to the TODO, so all its sessions share them. A worktree or PR registered in one session applies to every later session.
70. Session 1 starts with Start on a `not started yet` TODO (item 8).
71. Every later session starts with New session in the Focus view.
    - The button shows once the TODO has a session. While the TODO is `running` it is disabled, with the tooltip "Available once the agent stops"; otherwise its tooltip reads "End session n and start session n+1 with a hand-over".
    - Clicking it opens the Chat tab with the composer in new-session mode: a heading row `NEW SESSION n` with "⌘↵ starts session n. Session n−1 ends and stays above as history." and Cancel, the placeholder "First message for session n", the button "Start session", and a hint line with what is handed over (item 73): "Handed over: worktree, PR #n, <file names>", or "Nothing to hand over yet."
    - The draft and any attachments already picked stay in the composer. ⌘↵ or Start session starts the session at once; with an empty text the first message is "continue". The toast reads "Session n started" / "Session n−1 ended and stays above as history."
    - Cancel or Esc returns to the normal composer and keeps the draft. The mode is kept only in the browser: after a reload the composer is in normal mode, with the draft kept.
72. Starting session n (n > 1) does the following:
    - It sets `ended_at` on session n−1 and generates a new session id, as in item 9.
    - If the attention flag is raised, it clears it with actor `user`.
    - It records a `new session` event, which belongs to session n. Its detail's first line is `session n · <session id>`, followed by the hand-over header.
    - It sends the hand-over header, every queued message and the new message as one prompt. Queued messages go to the new session because the ended session gets no more runs.
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
    - Later turns of the session get only the worktree line (item 46).
74. The app does not write a "where I left off" note. If the user wants one, they ask the live session for it before starting the next. The note is a file like any other, so it appears in the next header.
78. Each earlier session shows its session id, so the user can resume it outside Switchboard by running `claude --resume <id>` from the project's local folder. That folder matters because Claude Code stores transcripts under the folder a session was launched from (item 44), as `~/.claude/projects/<cwd-slug>/<session-id>.jsonl`. The Claude ACP adapter runs Claude Code, which writes app sessions into that same store, so the CLI finds them (checked again in the ACP spike). Switchboard does not track such a resume.
10. A TODO's status is `not started yet`, `running`, `waiting on you` or `idle`. `running` and `idle` follow the run lifecycle (item 30). `waiting on you` lasts while the attention flag is raised. The agent clears its own flag; the user can also clear it with Clear attention. Each raise and clear is recorded with who did it (`user`, `agent` or `app`). A TODO has no "ended" state: `idle` is its resting state, and removing it is how it ends.
11. There is no pause action for a started TODO, and a started TODO never returns to the backlog. `not started yet` exists only before session 1; New session does not return a TODO to it.
12. A TODO can be removed in any status. The app asks first while the agent runs or while at least one of the TODO's terminals is alive. The dialog's body is built from "The agent is running. Removing interrupts it (Esc).", "N open terminal(s) will close." and "The TODO is hidden with its messages and files. Restore resumes the same session", which ends in "; terminals don't come back." when terminals close, otherwise in ".". The dialog's detail is the title. Every Remove, from any place, goes through this check. A Remove that needs no dialog acts at once and shows the toast "Removed" / "<title>. Restore it from Removed."
    - Removing interrupts a running agent, like pressing Esc in Claude Code; if the run has not ended 10 seconds later, the process is killed. It also ends the TODO's terminals (item 115).
    - The TODO gets `removed_at` and `removed_by` and disappears from the board or backlog together with its messages and files. Messages still queued at removal stay queued. The TODO's agent process ends: at once when it is idle, or when its interrupted turn ends.
13. The Removed view lists removed TODOs, newest removal first, with their project, who removed them and when, the live session's id with the number of sessions when there are 2 or more ("session 3b9d41f0 · 2 sessions", or "not started"), message and file counts, branch, a Draft badge and a Restore button. Counts use singular and plural ("1 TODO", "2 TODOs").
14. Restoring a removed TODO clears its removal, brings back its messages and files, and resumes its live session by session id (item 75). The prompt is the messages still queued at removal plus the user's uncommitted draft, sent as one prompt; with neither it is "continue". The "continue" is queued like any message, so a restore while the interrupted run is still ending goes out when that run ends. A TODO removed before it was started goes back to the backlog with its pending messages, nothing launches, and the toast reads "Restored" / "Back in the backlog." Terminals do not come back.
15. If the live session, the worktree or the project's local folder of a TODO is gone, restore shows a warning and does nothing else: it does not recreate the worktree or start a new session. The session check asks the agent: a short-lived agent process lists its sessions for the project's local folder (ACP `session/list`), and the live session id is not among them; the worktree and folder checks are the registered worktree path or the project's local folder not existing on disk. The warning text comes from the server and mentions that Claude Code deletes session transcripts after 30 days by default (`cleanupPeriodDays` in the user's `~/.claude/settings.json`).
75. A removed TODO keeps its sessions. Restoring it resumes the live session (item 14), and the checks of item 15 use that session's id. Removing a TODO does not end its live session; only New session ends a session.
16. A PR review is an ordinary TODO, e.g. "review PR #482" or a ticket reference.
46. When an agent creates a worktree, it registers the worktree's absolute path via `set_worktree`, and the app stores it on the TODO. While a worktree is registered, every later prompt starts with the line `Your worktree: <path>. Work there.`, because each run starts in the project's local folder, not in the worktree.
47. Every launch adds two folders to the session's additional directories: the project's worktree root, so the agent can work in the worktree and its `cd` persists across commands, and the TODO's artifact folder, which is outside the working directory and where the agent writes its files. The worktree root is `git -C <local_path> config worktree.root` if set, otherwise `<local_path>/../worktrees`; it is created if missing.

### Chat

17. Each TODO has a chat with the user's messages and the agent's messages, across all its sessions (item 76).
18. Agent chat messages are one or two lines at most. Longer content goes in a markdown file, and the message links to that file. `post_message` rejects longer messages, and links only a file already registered with `attach_file`.
19. The message composer follows a type, then commit flow (⌘↵). Nothing reaches the agent until the user commits. An uncommitted draft survives navigating away and reloading the page, and is shown with a Draft marker. Drafts are stored on the server, because restore uses them (item 14); the browser saves them 400 ms after the last keystroke and when the page is hidden. The placeholder reads "One line to the agent", or "One line to session n" when the TODO has 2 or more sessions.
20. A committed message is delivered according to the session state, and the composer's hint says which case applies:
    - When the agent is `running`, the message is queued and goes out as the next prompt as soon as the turn ends, together with any other message queued meanwhile. The hint reads "Agent is running. Your message goes out when its turn ends." The board's Running lane hint reads "Messages you commit reach the agent when its turn ends." ACP offers no way to hand a message to the running turn the way typing into Claude Code does (see `designs/acp/acp-spike.md`, item 20).
    - In the short moment between the end of a run's last turn and the end of the run, the message is queued and delivered when the run ends.
    - When the agent is `idle` or `waiting on you`, the message is delivered at once by resuming the live session.
    - When the TODO is `not started yet`, the message is stored and delivered at start.
    - Several messages waiting for the same delivery are sent as one prompt.
129. While the agent runs, the Focus header shows Stop (tooltip "Interrupt the agent, like Esc in Claude Code. A message it has not read yet is answered next."). Stop interrupts the turn like Esc in Claude Code (ACP `session/cancel`), records an `interrupted` event by `user`, and posts no error message. Messages queued during the turn (item 20) go out right after, as the next turn. If the turn has not ended 10 seconds after Stop, the agent process is closed (SIGTERM, then SIGKILL after 5 seconds). Stop while the agent process is still starting sends nothing: a new session is not opened and the messages stay as they were. There is no keyboard shortcut, because Esc closes things (item 59).
21. All messages are stored permanently with author and timestamp. Authors are `user`, `agent`, `user via operator`, `operator` (the operator's replies in its own chat) and `app` (messages the app itself writes into a TODO's chat, items 48, 49, 50). The chat marks queued messages, messages waiting for the start, and messages sent via the operator.
48. If a turn ends and the agent posted no message in it, the app shows the turn's final result text as the agent's message, cut to two lines and 280 characters. The full text is kept in the TODO's events.
49. If a run fails (the agent answers the prompt with an error, its process exits, or the turn ends with `refusal` or `max_tokens`), the app posts an `app` message naming the error, e.g. `Internal error: usage limit`, and the TODO goes to `idle` (or `waiting on you` while its attention flag is raised). A run the app interrupted itself (removal, shutdown) produces no error message.
50. When the permission system denies a tool call, the app posts an `app` message such as `Auto mode blocked Bash: git push --force origin main` and raises the TODO's attention flag with actor `app`. The agent may clear the flag after the user's reply, as with any other attention.
51. A `running` TODO shows its most recent tool on its card and in the Focus view, e.g. `tool · Bash`.
62. The user can attach images and other files to a TODO's first instruction and to later messages, by pasting, dropping or picking them. A follow-up may be attachments only.
    - Each file is saved to `<artifact folder>/uploads/<message id>-<n>-<name>`, and the prompt lists the absolute paths after the message text, so the agent opens them with `Read`.
    - Files waiting to be sent show as cards with a preview, their size and a × to drop them. They are kept only in the browser tab until the message is committed.
    - The chat shows sent images as thumbnails and other files as download links with their size. Attachments are served with `Content-Security-Policy: sandbox`, so an uploaded SVG or HTML file cannot run script.
    - A pasted image with the browser's generic name (`image.png`) is renamed `pasted-<HHMM>.png`. When the clipboard holds text and an image (a table copied from a document), the text pastes and the image is attached.
    - Files are sent as base64 inside the JSON body, because the server refuses non-JSON POSTs.
    - The operator chat takes no attachments.
76. The Chat tab shows the TODO's sessions as a timeline, oldest first. A TODO with one session shows no session header.
    - With 2 or more sessions, each session has a header: `SESSION n`, its label (the first line of the session's first user message), a `Live` badge on the live session, its time range ("Oct 3 · 09:12–10:40", or "Oct 6 · 10:02–" while live), and its session id with a copy button (tooltip "Copy id · claude --resume <id> from <project path>", toast "Session n id <id>").
    - The live session is expanded, with its chat and the composer below. Its header does not collapse.
    - Earlier sessions are collapsed to their header, `AGENT` with their last agent message (or "No agent message in this session."), and chips for the files registered in them; a chip opens the file in Files › TODO files. Expanding one shows the hint "Read-only here. Resume it outside the app: claude --resume <id>, run from <path>", its messages read-only, and its events when the Events switch is on (item 100). Which sessions are expanded is kept only in the browser.
77. A user message belongs to the session it was delivered to. An agent or app message belongs to the session it was posted in. Pending and queued messages show at the end of the live session.
100. The Focus view of a TODO has the tabs Chat, Files and Terminal, each with its count: `Chat N · Files N · Terminal N`. There is no Events tab. Instead, a Transcript switch next to the tabs, shown on a TODO's Chat tab only, mixes the TODO's transcript and events into the chat (item 130).
     - The switch reads "Transcript N" (the count of rows over all sessions), with the tooltip "Mix the transcript into the chat: what the agent writes, thinks and does, and the app's events. Applies to every TODO." It is off by default, and the pick is kept in localStorage.
     - Off, the chat shows messages only.
     - On, events are mixed in time order, and each run of events between two messages forms one block. A row shows the time, the kind with the first line of its detail, and the actor, e.g. `14:02  tool · Bash git status  agent`. Actor colours: `user` secondary, `agent` cyan, `operator` and `user via operator` violet, `app` muted. A detail with more than one line (the hand-over header, an `edited` project move) gets a fold arrow; a click opens the whole detail below the row.
     - `result` events (a turn's final text, item 48) always show open, as a box with light markdown and the footer of item 128.
     - With several sessions, each event shows in its own session's part of the timeline.
     - The composer stays below in both modes, and new events appear live, like messages.
130. **Transcript.** With the switch of item 100 on, the chat also shows what the agent does in a turn, written by the app from the agent's ACP updates (the agent's own transcript stays in Claude Code):
     - `say`: the agent's text between tool calls, as light markdown. The text after the last tool call is the turn's `result` (item 128), not a `say` row.
     - `thought`: the agent's thinking, one folded row.
     - `tool`: one row per tool call, updated in place: a status dot (cyan while pending or running, lime done, red failed, muted when the turn ended without a final status, as for a server tool like advisor), the kind (`read`, `edit`, `run`, `search`, …) and the title. A click opens its files (chips that open the file in the Files tab when it is inside the worktree or project folder), its input, its diffs (changed lines with three lines of context, from the editor bundle's line diff, at most 600 lines) and its output (cut to 8 KB).
     - `plan`: the agent's plan, one row per turn, open by default, updated in place. The card's and the Focus view's running line add `plan 3/7`. The plan is kept in `todos.plan` and cleared by New session. Claude sends a plan only when it uses its todo list tool.
     - While the agent writes, its current text (or the last line of its thinking) shows live at the bottom, marked `now`, from SSE `stream` events (`{type: "stream", id, item: {kind, text} | null}`, at most every 100 ms). It becomes a row when the next tool call starts or the turn ends.
     - Rows are `events` with `meta` (JSON): `{toolCallId, name, title, kind, status, input, locations, diffs, output}` for a tool, `{entries}` for a plan.
     - The operator's chat has no switch.
128. A `result` event in the chat ends with a footer that gives the run's duration and tool calls, e.g. "2 min 10 s · 9 tool calls". The duration is measured by the app from the prompt to the end of the turn; the tool count is the number of tool calls the agent reported in the turn. Both are stored on the `result` event.

### Files

22. Agents write plans, reviews, questions with context and designs as markdown files in the TODO's artifact folder, `~/.switchboard/data/<project-slug>/<todo-id>/`. The folder's absolute path is given in the app contract, so files survive worktree cleanup and stay out of git.
23. Agents register each file they create against their TODO via `attach_file`, which resolves symlinks and rejects any path outside the TODO's artifact folder. The Files tab's TODO files root (item 101) lists each file with its path, its line count ("1 line", "48 lines") or "missing on disk" (Open is then disabled), and, with 2 or more sessions, the session that registered it ("session 2"). A chat message that links a file shows a chip that opens it in Files › TODO files.
85. The TODO files list shows every file in the artifact folder except `uploads/` and `terminal/`, not only the files registered with `attach_file`, so files written from a terminal show up too. A file that no session registered is marked "added outside a session".
24. The user can open a file in the app, edit it inline, save and close it. Saving does not notify the agent. The user hands work back with a chat message such as "updated the review, post it but drop point 3".
25. A TODO file is read-only while its TODO's agent is `running`, and editable in any other status.
    - While locked, the file view says "Edit is locked while the agent is running. Read mode updates live as it writes." Clicking Edit shows the toast "Read-only" / "The agent is running. Edit unlocks when it stops."
    - If the TODO starts running while the user has unsaved edits (for example an operator message or a queued delivery resumes it), the edits stay in the browser and the editor locks. Saving is allowed again once the TODO stops running. Unsaved edits also survive closing the file: it reopens in Edit with them.
    - If the file changed on disk since it was loaded, saving opens the dialog "Changed on disk": "<path> changed on disk after you opened it. Overwrite replaces it with your version. Discard drops your edits and reloads the file." Its detail shows both times (`loaded   mtime 10:41:07`, `on disk  mtime 10:43:52`), and its buttons are Keep editing, Discard and Overwrite.
    - An open file without unsaved edits reloads when the file changes on disk.
26. Files are stored on disk. The database stores only the mapping between files and TODOs.
52. A markdown file's Read mode shows the rendered markdown. Its Edit mode uses the code editor of item 107 with a formatting toolbar (heading, bold, italic, quote, link, code, bullet list, numbered list, task, rule, table), a Write / Split / Preview switch, where Split shows the editor and the rendered preview side by side, and a status bar with the line and word counts. Each toolbar button inserts markdown syntax around the selection.
101. The Files tab has a root switcher:
     - **TODO files**, the default, with its count: the artifact files of items 22 to 26 and 85.
     - **Worktree**, labelled with its branch: the TODO's registered worktree. It shows only while a worktree is registered and exists on disk.
     - **Project**, labelled with the project's name: the project's local folder, which is the main checkout.

     Each root's tooltip gives its path. A note on the right names the root's folder and what it lists: "<artifact folder>/ · terminal/ and uploads/ not listed", "<worktree path> · git ls-files · refreshed every 5 s", or "<project path> · main checkout". Picking a root closes the open file. The pick is kept per TODO in the browser until the page is reloaded, and falls back to TODO files when the worktree is gone.
102. The Worktree and Project roots show a folder tree of the files git knows about: tracked files plus untracked files that are not ignored (`git ls-files --cached --others --exclude-standard`). Ignored files such as `node_modules` and build output are not shown, with one exception: the root folder's own symlinks are always listed, even when git ignores them, because the worktree skill links `.env` and `CLAUDE.local.md` into a worktree that way. Symlinks are tagged "→ main folder" (tooltip "Symlink into the main folder, made by the worktree skill").
     - Folders come first, then files, sorted case-sensitively. Folders expand on click, Enter or Space.
     - Folders that contain changed files start open. After that, the open state is kept per TODO and root.
103. The tree marks changed files: `M` modified (amber), `A` added (green), `U` untracked (cyan) and `D` deleted (red). The name takes the mark's colour, and a deleted file is struck through. A collapsed folder with changes shows an amber square ("Contains changes").
     - The marks combine uncommitted changes (`git --no-optional-locks status --porcelain=v2`) with the changes committed on the branch since its merge base with `origin/HEAD`, the same base the card's git line uses (item 5). Both roots use the merge base of that folder's `HEAD` with `origin/HEAD`.
     - A **Changed** toggle (Worktree and Project only, tooltip "Uncommitted changes plus commits since the merge base with origin/HEAD") lists only these files, flat, each with its parent path, and shows their count. Deleted files appear only there. The toggle is one setting for every TODO and root. With nothing changed, the list says "No changes on this branch." (Worktree) or "No uncommitted changes in the main folder." (Project).
104. Opening a file shows it in the file view inside the Files tab. The Chat, Files and Terminal tabs and the root switcher stay visible, and "← Files" goes back to the list. The header shows the path (absolute for TODO files, relative to the root for tree files), the size and line count ("2.1 KB · 48 lines"), the mark for tree files, and the badges "Read-only · agent running" and "Unsaved".
     - Markdown files: item 52.
     - Other text files, including TODO files that are not markdown, show syntax-highlighted source with line numbers, without wrapping.
     - Images show centred and scaled down to fit, served with `Content-Security-Policy: sandbox` as attachments are (item 62).
     - Binary files show "Binary file · 12.4 KB. Not shown." and have no modes. A file with a NUL byte in its first 8 KB is binary.
     - Text files over 2 MB show "Large file · 3.4 MB. Not shown."
     - A path the server refuses, a file that vanished, or a read error shows "Can't open: <reason>", plus a toast.
105. A file outside TODO files that is marked, was saved in this page, or has unsaved edits also has a **Diff** mode. It compares the merge-base version with what the editor shows: the unsaved draft if there is one, otherwise the file on disk. It shows the whole file, with old and new line numbers and added and removed lines tinted, and no syntax colours. It is read-only and stays available while Edit is locked. Its footer says what it compares: "Diff · merge-base <sha> (origin/HEAD) → on disk · read-only", or "… → your unsaved edits". A deleted file opens in Diff only.
106. The file view has **Copy path** and **Add to message**.
     - Copy path copies the absolute path, with the toast "Copied" and the path.
     - Add to message appends the path to the TODO's message draft: relative to the worktree for worktree files, absolute for Project and TODO files. Its toast "Added to message" has a Go to chat action (item 125). The file stays open in the Files tab.
107. Read and Edit modes use a code editor (CodeMirror 6) for every file, including TODO files. Read mode is the same editor, read-only, except for markdown, which reads rendered (item 52). ⌘E switches between Read and Edit, ⌘S saves, and ⌘F opens the editor's search in Edit mode (item 59). The key hints read "⌘E edit ⌘S save ⌘F find".
     - Editing is light on purpose: there is no creating, renaming or deleting of files, no search across files, no LSP and no git actions. The agent or a terminal does those.
108. Locking and conflicts depend on the root:
     - **TODO files**: item 25.
     - **Worktree files**: read-only while the TODO's agent is `running`, as in item 25. Read mode follows the file as it changes.
     - **Project files**: editable at any time. TODOs without a worktree may work in the main folder, and the app cannot tell which files they touch, so the conflict check is the guard here.
     - **Every save** sends the mtime the file was loaded with. If the file changed on disk since then, saving opens the "Changed on disk" dialog of item 25.
     - Saving does not notify the agent (item 24). The toast reads "Saved" / "<path> written to disk. Nothing sent to the agent." The save records a `file saved` event with actor `user`: `file saved · <name> · agent not notified` for a TODO file, `file saved · worktree · <path> · agent not notified` for a worktree file, `file saved · project · <path> · agent not notified` for a project file.
     - A footer under the file says what saving does: "Saving writes to disk only. The agent isn't notified. Tell it in chat when you want it to act." (TODO files); "Read-only while the agent runs. Read mode follows the file as it changes." (worktree, running); "Saves to the worktree. The agent isn't notified. A save warns if the file changed on disk since you opened it." (worktree, not running); "Main checkout. Editable any time; a save warns if the file changed on disk since you opened it." (Project).
109. The tree and the open file refresh in two ways. While a Worktree or Project root is visible, git is polled every 5 seconds. The TODO's runs also trigger a refresh when they end.
     - An open file without unsaved edits reloads when it changes on disk.
     - There are no file-system watchers. A recursive watch on Linux takes one inotify watch per folder, including ignored ones like `node_modules`, and polling git behaves the same on every OS.
110. File requests name a root and a path relative to it.
     - The server resolves the real path and accepts only paths inside the project's local folder or its worktree root (item 47). Symlinks the worktree skill makes, such as `.env` or `CLAUDE.local.md` pointing into the main folder, therefore open, and nothing outside the project opens.
     - Writes go to the resolved file, so symlinks are kept.
     - Writes inside `.git/` are refused.

### Terminals

82. A terminal belongs to a TODO. It opens in the TODO's worktree, or in the project's local folder when no worktree is registered (item 111).
83. Whatever runs in a terminal, including `claude`, is the user's own process, like a Claude Code session started in another terminal app. It is not a session of the TODO:
    - it does not change the TODO's status, attention or chat;
    - it does not appear in the timeline;
    - the one-live-session rule of item 68 does not limit it.

    This makes a terminal the untracked way to work in parallel inside one TODO. A second TODO is the tracked way.
84. A terminal's environment includes `SWITCHBOARD_TODO_ID` and `SWITCHBOARD_FILES` (the TODO's artifact folder), so a `claude` started there can be told to read or write the TODO's files.
111. The Terminal tab holds any number of terminals for the TODO, each in its own tab with a × to close it ("Close terminal"). The Terminal tab's count includes exited terminals. A TODO that has not started has a Terminal tab too.
     - A **+** button opens a menu with **Shell** ("git, docker, tests, a dev server") and **Claude** ("New session that starts with this TODO's context", item 113), under the heading "Worktree · <branch>". When there is a worktree, the menu offers both again under "Project folder · <path>" (Shell: "git or tools on the main checkout"), for example for a git command on the main checkout. Without a worktree, only the project folder is offered. A Shell opens at once; Claude opens the form of item 113. The menu closes on a pick, on Esc, on a click outside it, and when switching TODO.
     - Without terminals the tab shows "No terminals", "Opens in <path>. A shell for git, tests or a dev server, or Claude with this TODO's context. Both are your own processes: the agent isn't told about them and nothing shows in the chat.", and the buttons New shell and New Claude.
     - A note next to the terminal tabs reads "N terminals · your own processes · the agent isn't told about them", or "Your own processes · the agent isn't told about them" with none.
     - Each terminal tab's tooltip is its folder. A footer under the terminal names the program ("zsh · login shell", using the real shell's name, or "claude · separate session") and the folder, and says "Keys go here while focused", or "Click outside to hand keys back" while it has focus.
112. A terminal tab is labelled with the title its program sets (shells and Claude Code set one). Without a title it reads `Shell N` or `Claude N`. The numbers count up per kind and are never reused: after closing `Shell 2`, the next shell is `Shell 3`. A terminal in the project folder while a worktree exists also gets `· project`. An exited terminal shows `exit <code>`.
113. **Claude with context.** New Claude opens a form, headed "New Claude" and "Separate session · your settings · no app tools or hooks", with the folder (read-only) and an editable first message with the hint "Add your own ask, or keep it and give instructions once Claude has read the context." The message is prefilled with:

     > I'm working on the Switchboard TODO "&lt;title&gt;". What its agent is doing, its branch and PR, the recent chat and its files are in `<context file>`. Read it, then wait for my instructions.

     The form shows the command it will run (`cd <folder>`, then `claude --add-dir <artifact folder> "<first message>"`) and notes that it writes the context file first. ⌘↵ or Start starts; an emptied message falls back to the prefilled text. The app then:
     - writes the context file (item 114);
     - runs `claude --add-dir <artifact folder> -- "<first message>"` in the chosen folder. The `--add-dir` lets Claude read and write the TODO's files without permission prompts; the `--` ends its folder list, which would otherwise take the message as a second folder.
     - Claude Code asks once whether it may trust a folder it has not seen before, as in any terminal; the first message follows the answer.

     This is a new session, separate from the TODO's:
     - It runs with the user's normal Claude Code settings and permission mode, without the app's MCP tools or hooks.
     - Nothing it does appears in the TODO's chat, and the app does not store its session id. Claude Code keeps its transcript as for any session, so `claude --resume` in a shell finds it.
114. The context file is `<artifact folder>/terminal/context.md`, rewritten at each Claude launch and restart. The Files listing skips `terminal/`, as it skips `uploads/` (item 85). It is a snapshot, taken at launch, of:
     - the TODO's title, project, status (`not started` for a TODO that has not started) and first instruction;
     - the worktree path, branch, git line and PR line (item 5);
     - the whole chat of every session, with author, time and session headers. Agent messages are one or two lines (item 18), so even a long chat stays small;
     - the TODO's files, with absolute paths, so Claude can read them;
     - the session ids of the TODO's sessions, so Claude can find their full transcripts under `~/.claude/projects/` if it needs more history;
     - a note that the TODO's agent may be working in the same worktree at the same time, so Claude checks with the user before committing, switching branches or rewriting history.
115. A terminal's lifetime:
     - It keeps running when the user switches tabs or TODOs, closes the browser tab, or reloads. Reopening shows its recent output, then the live terminal. While the connection is down, the footer shows "Disconnected · reconnecting…".
     - It ends when its tab is closed, when the TODO is removed (item 12), or when the app shuts down (item 56). Closing a tab whose process is alive asks first: "Close terminal", "The shell in this tab is still running. Closing the tab ends its process." (or "Claude in this tab is still running. …"), with the folder as detail and the buttons Cancel and Close terminal. An exited tab closes without asking.
     - When its process exits, the tab stays with "Process exited with code N." (a Claude tab adds " Its transcript stays; claude --resume finds it."), or "Process ended by signal SIGTERM." when a signal ended it. It offers **Restart**, and a Claude tab also **Shell here**, a new shell in the same folder.
     - Restart reruns the program in the same tab. A Claude terminal reruns with the same first message after the context file is rewritten, and its label goes back to `Claude N`.
     - A tab the page still shows after the server restarted reads "Ended: the app restarted."
     - A program that cannot start (for example, the worktree was deleted after a merge) leaves the tab exited, and the error shows as a toast.
     - Restoring a TODO does not bring its terminals back.
116. The programs:
     - **Shell**: the shell named by `SWITCHBOARD_SHELL`, else `$SHELL`, started as a login shell (`-l`).
     - **Claude**: `claude`, found on PATH and run directly, not through a shell, so shell aliases for `claude` do not apply. In the window app, PATH comes from the login shell (item 66).
     - **Environment** for both: the server's, plus `TERM=xterm-256color`, `COLORTERM=truecolor`, `SWITCHBOARD_TODO_ID` and `SWITCHBOARD_FILES` (item 84).
     - Terminals need Bun 1.3.5 or newer, on macOS or Linux. Otherwise the Terminal tab says "Terminals need Bun 1.3.5 or newer. This server runs Bun x." or "Terminals are not supported on Windows yet."
117. Keys and clipboard:
     - While a terminal has focus, every key goes to it, including Esc, Tab and N. On macOS ⌘ combinations still reach the app, so the ⌘ shortcuts keep working. On Linux and Windows only Alt+J and Alt+, reach the app; other Alt combinations go to the terminal, where Alt is the shell's Meta key (Alt+B, Alt+F, Alt+.).
     - On Linux, Ctrl combinations go to the terminal (Ctrl+J, Ctrl+E and Ctrl+S mean something there). Copy and paste are Ctrl+Shift+C and Ctrl+Shift+V.
     - Clicking outside the terminal hands the keys back to the app.
     - Terminal tabs are selected by click, Enter or Space.
118. The TODO's agent does not see the terminals and is not told about anything done in them. Shells, Claude sessions and the agent can work in the same worktree at the same time, as in any terminal. Nothing guards against two of them running git at the same moment (see "Not built on purpose" on shared worktrees). For Claude, the context file says so.
119. Files and terminals belong to a TODO, also one that has not started. The overview, the operator, removed TODOs and Settings have neither.

### Access

120. The app requires a **launch token**, a random secret the server picks at each start. Every request under `/api/` must carry it, including the SSE stream, attachment and image URLs, and the terminal WebSockets; without it the server answers 401. The page itself (`/`), the favicon and `/vendor/*` stay open, because they hold no data.
     - The window app gets the token itself, also when it attaches to a server that is already running (item 65), so nothing changes for the user.
     - In browser mode, `bun server.ts` prints `http://127.0.0.1:4477/#token=…` at start (item 64). Opening that link stores the token in the page, so reloads keep working. After a server restart the old token stops working, and the page shows one full-page message: "This page needs a new link. The server prints it when it starts (`bun server.ts`); the window app opens it on its own."
     - Agents do not get the launch token. They reach the app's MCP tools at `/mcp/app` with a secret of their own (item 28).
     - This shuts out programs that can reach the port but cannot see the token. Examples are another user's programs on a shared machine, or a Linux Docker container started with `--network host`. Without the token such a program could create and start TODOs; with terminals it would get a shell directly.

### Agent launch and app contract

27. Agents run in auto permission mode (`permissionMode: 'auto'`).
28. Agents run over the Agent Client Protocol (ACP): the app starts the agent (`claude-agent-acp`, on the Bun that runs the server) and speaks JSON-RPC to it over stdio. The app's MCP server is passed in the ACP session's `mcpServers`, as an HTTP server at `http://127.0.0.1:<port>/mcp/app` with an `X-Switchboard-Session` header, not via the repository's settings, so only app-launched agents report to the app and the user's normal Claude Code sessions in the same repo are unaffected.
    - The header holds a random secret, 32 bytes, made for each agent process and dropped when it ends. The MCP config reaches the Claude CLI in its argv, which other users can read in `/proc`, so the config only says `${SWITCHBOARD_MCP_SECRET}` and Claude Code expands it from the agent process's environment, which only the same user can read. The secret names the TODO (or the operator), so each tool call knows its TODO without matching session ids. A request without a known secret gets 403.
    - `/mcp/app` is stateless: each POST gets a fresh MCP server (`WebStandardStreamableHTTPServerTransport` with JSON responses); GET and DELETE get 405. The Host and content-type checks of item 120 apply as everywhere.
29. Worker agents have these MCP tools: `post_message`, `set_title`, `set_attention` (raise and clear), `attach_file`, `set_worktree`, and `set_pr` (the pull request the TODO works on, also without a worktree; otherwise the PR is found by the worktree's branch). The contract names them by their full tool ids (`mcp__app__post_message` etc.), because with the user's many MCP servers the agent loads tool definitions on demand.
30. Status follows the run lifecycle:

    | Event | Status |
    |---|---|
    | A run starts (start, resume, queued delivery, new session), from the moment its agent process starts | `running` |
    | The run ends, normally, with an error, or by interrupt | `idle`, or `waiting on you` if the attention flag is raised |

    The agent's `tool_call` updates record `tool` events and the most recent tool (item 51). A run is one turn, and the turn's end (the `session/prompt` response) is the idle signal, also after an interrupt.
31. Every agent receives a fixed app contract that the user cannot edit, appended to the system prompt before the project prompt. Settings › Projects shows it read-only, at full width. Its first line says the session is part of a TODO that can span several sessions. The contract instructs the agent to:
    - talk to the user only through the app's MCP tools, and set a title on its first turn of session 1 only;
    - keep chat messages to one or two lines and put anything longer in a file under the given artifact folder, linked from the message;
    - treat the TODO text and the user's committed messages as its go-ahead for local edits within this TODO's scope, while outward actions (push, PR comments, Jira, Slack) still need an explicit message from the user;
    - work out from the TODO text, and from any referenced ticket, what kind of work this is (a review, a feature, or thinking only with no code changes) and whether it needs a worktree, following the project prompt for how to create one;
    - after creating a worktree, call `set_worktree` with its absolute path and work only there, ignoring any skill instruction not to switch context;
    - when the prompt starts with the hand-over header (item 73), keep the title, work in the registered worktree instead of creating a new one, and read whichever listed files it needs before starting;
    - call `set_pr` as soon as it knows the TODO works on an existing GitHub pull request, also without a worktree;
    - ask, via a message plus the attention flag, rather than guess when anything is unclear;
    - post a message and raise attention when auto mode blocks an action or it is unsure how to proceed, instead of routing around the block;
    - clear its own attention flag once the user's reply resolves it.
32. There is no dispatcher or orchestrator pass. A started agent interprets its own TODO.
53. The app has no model setting. Agents use the model from the user's Claude Code settings.

### Operator

33. A pinned operator card sits on the TODOs screen: next to the New TODO card in the backlog, and alone at full width on the board. It shows the operator's status and its last two messages, with replies rendered as markdown. Its input sends on Enter (Shift-Enter adds a line). Opening the card shows the operator in the same chat UI as a TODO, where ⌘↵ commits and the placeholder reads "Ask about TODOs, or tell it to create or message one".
34. The operator has these tools: `list_todos` (titles, projects, statuses, attention, branch, git and PR state, files and latest messages, optionally removed TODOs), `read_todo` (a TODO's details, sessions, messages and file list, removed TODOs too; item 81), `read_file` (a TODO's file by name), `create_todo` and `send_message`, plus Claude Code's Read, Grep, Glob, WebSearch and WebFetch. It has no Edit, Write, Bash, none of the user's MCP servers, and no tool to start TODOs; anything else is refused (`permissionMode: 'dontAsk'`). It runs with `~/.switchboard` as its working directory.
81. The operator's tools say "todo", not "session", because a session is a part of a TODO. `read_todo` returns the TODO's sessions (number, id, label, start and end time) and its messages grouped by session. The operator still never starts a TODO or a session (item 35).
35. The operator acts only on the user's explicit instruction. It never polls, never answers a worker's question on its own, and never starts TODOs.
36. TODOs created by the operator start as `not started yet` and land in the backlog, where their row shows an `Operator` badge (item 88). The user starts them.
99. The operator's system prompt says "backlog" for TODOs that have not started, so its answers match what the user sees. `list_todos` needs nothing extra: the status tells backlog from board. The operator has no tool to edit or start backlog TODOs (item 35).
37. A message the operator sends on the user's behalf appears in the target TODO's chat as a user message with a "via operator" marker, and is delivered like a message the user commits (item 20).
38. The operator gets session state through its tools rather than from its conversation, so the user can start a fresh operator session at any time with New session without losing anything. A fresh session gets a new session id and hides older operator messages.
39. Brainstorming, such as thinking through a future feature, works in the operator or in a TODO the agent treats as thinking only.
54. Operator replies are shown in full, rendered as markdown. The one-or-two-line rule (item 18) does not apply to the operator.
127. Above each operator reply, a violet line lists the tools the operator called for it, e.g. `→ list_todos()` or `→ create_todo(project: "infra")`. It is built from the operator's tool events between the previous reply and this one. The operator's header lists its tools as chips: `list_todos`, `read_todo`, `read_file`, `create_todo`, `send_message`.

### Attention

40. The board shows started TODOs in three lanes, in this order: `Waiting on you`, `Running`, `Idle`. Within each lane, the most recently updated TODO comes first. TODOs that have not started are in the backlog (item 88), not on the board.
41. The browser tab title shows the number of TODOs in `waiting on you`, e.g. `(2) switchboard`. Only `waiting on you` counts.
55. A TODO shows an Unread badge when an agent message arrived after the user last opened that TODO; opening it clears the badge. Unread TODOs are not counted in the tab title and do not move to the waiting lane.

### Persistence

42. A local SQLite database stores projects, TODOs and their sessions, messages, drafts, attachments, file mappings, events (attention changes, runs, tool calls, denials, edits, file saves, and each run's duration and tool count), each live session's context usage, and removals (`removed_by`, `removed_at`). Messages, events and files record the session they belong to. This history stays available for the operator after a TODO is removed. Terminals are not stored.
56. On shutdown (Ctrl-C, SIGTERM, or quitting the window app), the app interrupts all running agents, ends all terminals and waits up to 5 seconds; a second Ctrl-C exits at once. On startup, any TODO still marked `running` is set to `idle` (or `waiting on you` while its attention flag is raised) with an `app restarted` event. Messages still queued stay queued. A Deliver bar ("N queued messages not delivered yet." with Deliver) shows whenever queued messages wait and the agent is not running. Nothing is delivered automatically on startup.
57. On first start (no database yet), the app imports the projects (name, local path) from `~/.auto-coder/db.sqlite`, leaves the old reviews and findings behind, and writes the old stored review prompt to `~/.switchboard/old-review-prompt.md`, so the user can paste it into project prompts.

### UI

58. The screens:
    - the title bar (item 121), with the `Backlog`, `Board` and `Removed` tabs, a Settings tab while Settings is open, the PR status chip and the Settings gear;
    - the TODOs screen, Board or Backlog (item 86), with the pinned operator card, the project filter row, and the board's lanes or the backlog list;
    - the Focus view of a TODO: the title, the info line (item 124), the Chat, Files and Terminal tabs (item 100), its "1 of 2 waiting" counter and the next waiting TODO;
    - the Removed view;
    - Settings, with Projects and Appearance (item 122).

    `Backlog` counts not-started TODOs and `Board` started ones, across all projects. `Removed` counts removed TODOs. The project count sits in the Settings nav.
121. **Title bar and window.** The top of the app is a title bar with the brand mark and wordmark, the view tabs, and on the right the PR status chip and a Settings gear (tooltip "Settings · ⌘,", outlined while Settings is shown).
     - The tabs read `BACKLOG n | BOARD n | REMOVED n`. `Backlog` counts live not-started TODOs and `Board` live started ones, in all projects whatever the filter. While any TODO waits, an amber square before `Board` carries the tooltip "N waiting on you". The Focus view sits under the tab it was opened from. Clicking that tab from Focus returns to its overview; clicking it from Settings or Removed returns to Focus. Tabs activate on click, Enter or Space.
     - The PR status chip shows while `gh` fails. Its tooltip reads "PR lines show the last result from N min ago, greyed. Git lines are local and stay current. Retries every 2 min. Click to retry now." Clicking it retries.
     - In the window app on macOS, the title bar replaces the window frame: the native traffic lights sit inside it, a spacer keeps them clear of the brand, and dragging the bar moves the window. In full screen and in the browser there is no spacer. The page knows it runs in Electron from its user agent.
     - On Windows and Linux the window keeps its native frame, with the title bar below it.
     - The window is at least 960 pixels wide. Its app menu has "Settings… ⌘," (Alt+, on Linux and Windows) plus the standard Edit, View and Window menus, so ⌘C and ⌘V keep working.
     - Icons: the macOS set (`images/icons/macos/`) for the packaged app and the dock, the flat set (`images/icons/flat/`) for the window icon on Windows and Linux and the browser favicon, and the cut-corner brand mark in the title bar.
122. **Settings** opens with the gear or ⌘, (also while typing), as a closable tab after Removed. It reopens the last section shown. × (tooltip "Close · Esc") or Esc, when not typing, returns to the view Settings was opened from. A nav lists the sections `Projects <n>` and `Appearance`.
     - Projects: item 1, with the app contract read-only at full width (item 31).
     - Appearance: a Theme choice of Auto, Dark and Light (tooltips "Follow system · now dark", "Dark theme", "Light theme"). Auto follows the OS setting, and the hint says which theme that is now ("Auto follows the system setting. Now dark."), also when the OS setting changes. Dark and Light say "Fixed to dark. Applies to every window." and "Fixed to light. Applies to every window." The pick is kept in localStorage.
     - Without projects, the New TODO card says "Add a project in Settings › Projects first."
86. The TODOs screen has two sides, **Board** and **Backlog**, picked with the title bar tabs (item 121): `BOARD n` (tooltip "Started work · B") and `BACKLOG n` (tooltip "Not started yet · B"). `B` toggles them on the overview, and from Removed or Settings returns to the side last shown. The pick is kept in localStorage; the board is the default.
    - The board shows the pinned operator card at full width, the project filter row and the lanes. The board has no New TODO card.
    - The backlog shows the New TODO card next to the operator card, the project filter row and the backlog list.
87. The **board** shows the lanes of item 40. When no started TODO passes the project filter, one panel replaces the lanes: "No started TODOs[ in <project>]. <n> idea(s) in the backlog.", or "No started TODOs. The backlog is empty too.", with an Open backlog button.
88. The **backlog** lists every TODO that has not started, most recently touched first, as compact rows rather than cards, so many ideas fit on one screen. Its header reads `BACKLOG` with the count and the hint "Recently touched first. A note or an edit moves an idea to the top."
    - A row shows the project and the age in short form (`now`, `12m`, `4h`, `1d`, `2w`, `3mo`); the first instruction, clamped to two lines; a line with the notes and attachments waiting for the start ("2 notes · 1 attachment · waiting for the start"); an `Operator` badge when the operator created it; and a Draft badge.
    - Each row has **Start** and **Remove** (tooltip "Remove · no confirmation, nothing is running"). Remove goes through the check of item 12, so a backlog TODO with a live terminal still asks.
    - Click, Enter or Space on a row opens the Focus view.
    - An empty backlog says "The backlog is empty. Capture an idea above.", or "Nothing in the <project> backlog." while a project is picked.
89. Starting a TODO from the backlog leaves the user in the backlog, so they can keep browsing. The toast "Started" / "<first 48 characters>. It is on the board now." has an Open action. Starting from the Focus view stays in the Focus view.
90. The project filter (item 63) applies to both views. Its chip counts and the "<n> TODOs in other projects" line count the TODOs of the current view. The amber squares, "N waiting on you" and "· N waiting" count all live TODOs, because waiting is the signal that matters wherever the user is.
63. The TODOs screen has a project filter row above the lanes or the backlog list: an All chip and one chip per project, each with its count (item 90) and, for a project with TODOs waiting on you, an amber square. Picking a project shows only its TODOs, limits `⌘J` and the Focus "1 of 2 waiting" counter to that project, and preselects it in the New TODO card. While a project is picked, the row says how many TODOs (and how many of them waiting) are in other projects, with a Show all button. The title bar count, the Board amber square and the tab title still count all projects. The pick is kept in the browser's localStorage.
91. The Focus view's back button returns to the view the user came from and is labelled after it ("← Board", "← Backlog"). Esc goes back the same way, and `⌘J` keeps that origin. `⌘J`, the "1 of 2 waiting" counter and the tab title count only concern `waiting on you`, which only exists on the board.
93. `N` focuses the New TODO input. From the board or the Focus view it first switches to the backlog.
124. **Focus header.** The Focus view's header shows the title first, then one info line with parts separated by `·`. A part with no value is left out, with no `—` or `none`:
     - the project (a select before the start, item 94);
     - the session (item 79), with a copy button (tooltip "Copy session id · resume outside with claude --resume", toast "Copied" / "Session id <id>");
     - the branch;
     - a git summary: the first amber or red fact (else the first fact) plus `+N` for the rest. Hovering or focusing it opens a popover `GIT · <branch>` with every fact in its colour, including how far the branch is behind its base, and the footer "local · uncommitted plus commits since the merge base with origin/HEAD";
     - "project folder, shared with other TODOs" when there is no worktree;
     - the not-started note (item 94).

     On the right: the PR as `PR #n ↗`, a link to GitHub, with the first fact after its state and `+N`. Its popover shows `PR #n · <state>`, every fact, and the footer "registered by agent via set_pr · checked N min ago", or "as of N min ago" with muted facts when `gh` fails. Without a PR the part reads "no PR · branch pushed" or "no PR · as of N min ago". Then the context meter (item 123). When `gh` fails, its reason and Retry now sit on their own line below.
     - The action buttons are the status badge, Start, Stop (item 129), Clear attention, New session (item 71) and Remove.
     - The operator's info line reads `operator · all projects · session <id>`, with a copy button (toast "Operator session id <id>") and the context meter. Below it, the intro "Answers about every TODO and acts only when you tell it. It can create TODOs and message them, never start them." shares one row with the tool chips (item 127).
79. The info line shows `session <id>` for a TODO with one session and `session n of n · <id>` with 2 or more. The TODO files list shows the session that registered each file (item 23), and events in the chat sit in their session's part of the timeline (item 100).
123. **Context meter.** At the right end of the Focus info line, for a TODO's live session and for the operator, a meter shows how full the context window is: `CONTEXT`, a bar and `142k / 200k`. The bar is cyan below 60%, amber from 60% and red from 85%; the text takes the colour from 60%.
     - Hovering it opens a popover headed `Session n · context` (plus " · updates after the turn" while running; "Operator session · context" for the operator), with "N of W tokens · P%", "last turn: Xk in · Yk out", "N turns in this session", the advice and the note "Read from the last model call (input + cache tokens)."
     - The advice reads "Plenty of room." below 60%, "Getting full. A good point for a new session once this step is done." from 60%, and "Close to full. Claude Code compacts on its own soon and older detail gets summarised. Start a new session if this one has done its job." from 85%.
     - The numbers update after each run. The window size comes from the agent, not a fixed 200k. New session clears them, and the meter is hidden until there is usage.
125. A toast can carry one action button. Clicking it runs the action and closes the toast. A toast with an action stays 6.5 seconds, others 4.2 seconds, and at most 3 toasts show at once. The actions are Open on "Started" (opens the TODO's Focus view) and Go to chat on "Added to message" (opens the Chat tab with the draft).
59. Keyboard shortcuts:
    - The ⌘ shortcuts below use Alt instead on Linux and Windows, where Ctrl belongs to the browser and the terminal (Alt+J, Alt+E, Alt+↵, Alt+⇧↵ and Alt+, for Settings); Ctrl does not trigger them there. The page tells from `navigator.platform` and shows its key hints and tooltips the same way. Paste stays the system's key, so the image hints read Ctrl+V there.
    - `N` switches to the backlog and focuses the New TODO input (item 93). `B` toggles Board and Backlog, only when not typing, with no modifier, on the TODOs screen and not in the Focus view.
    - `⌘J` opens the next TODO waiting on you.
    - `⌘,` opens Settings, also while typing.
    - `⌘E` toggles file edit mode, `⌘S` saves the open file, and `⌘F` searches it in Edit mode. All three work only while the file is visible.
    - `⌘↵` commits the draft, or adds the TODO in the New TODO card, saves the first-instruction edit, starts the new session, or starts the New Claude form. `⌘⇧↵` adds and starts a TODO (item 92).
    - `Esc` closes one thing, in this order: a dialog, the terminal + menu, the New Claude form, the first-instruction edit, new-session mode, Settings (when not typing), the open file, and finally the Focus view, back to Board or Backlog.
    - Enter or Space opens a focused card or backlog row, and activates view tabs, terminal tabs and tree rows. Arrow keys move within the radio groups (project filter, Files root, Theme).
    - In dialogs, Esc closes the dialog, other keys are blocked, and focus stays inside.
    - While a terminal has focus, keys go to it (item 117).
60. The design's "Show flow" panel (scenarios, lifecycle trace, live event feed) and the "Launch · per turn" command preview in Settings › Projects are not built. Show flow plays the design's demo scenarios, not the app's data. The launch preview shows a command line such as `claude -p … --settings hooks.json --mcp-config …`, which is not how the app launches agents: it starts the ACP adapter and passes the MCP server in the ACP session (item 28), so there is no such command to show.
61. The UI gets live updates from the backend over one SSE stream, including terminals opening, closing, exiting and changing title. After a reconnect it refetches what is on screen.

## How it is built

- **Shape.** One local Bun process (`server.ts`) owns the agent runs (`runner.ts`, over ACP in `acp.ts`, with the app's MCP tools in `apptools.ts`), the SQLite database (`bun:sqlite`), the terminals, and an SSE stream for the browser UI. Pure helpers live in `lib.ts`, covered by `bun test`. The frontend is a single `index.html` with no build step; its libraries and fonts come from `vendor/`. The Electron window (`electron/main.js`) only wraps the unchanged Bun server, because Electron runs Node and the server needs Bun.
- **Vendor bundles (item 126).** `bun run vendor` builds these from entry files in `vendor/src/`. All are minified and committed.
  - `vendor/markdown.js`: marked and DOMPurify as globals, loaded at start.
  - `vendor/editor.js`: CodeMirror 6 (`codemirror`; `@codemirror/lang-*` for markdown, JS/TS, JSON, CSS, HTML, Python and YAML; `@codemirror/legacy-modes` for shell and HCL-like files; `@codemirror/merge` for its `diff()`), loaded on first use.
  - `vendor/terminal.js`: xterm.js 6 (`@xterm/xterm` with `addon-fit`, `addon-webgl` and `addon-web-links`), loaded on first use.
  - `vendor/fonts/*.woff2` and `vendor/fonts.css`: the Latin subsets of Chakra Petch, Space Grotesk and JetBrains Mono, weights 400–700, from `@fontsource/*`, about 250 KB.

  The server serves `/vendor/*` without the token, after a path check (`isInside`), and the Electron `build.files` list includes `vendor/**`. One bundle guarantees one copy of `@codemirror/state`; loading CodeMirror's packages separately from a CDN can produce two copies, which breaks its extensions. Sizes: about 90 KB gzipped for xterm.js, and about 125 KB plus the languages for CodeMirror. Not Monaco: its ESM build needs a bundler that handles CSS imports, it is about 2.5 MB, and it is built for a full editor, which this is not. Not bytemd, the editor the design draws: it runs on CodeMirror 5, handles only markdown and is about 650 KB, so code files would need a second editor with a second theme.
- **Loopback only, with a launch token.** The server binds to 127.0.0.1. It accepts only requests whose Host header names this server (`127.0.0.1:<port>` or `localhost:<port>`), which blocks DNS-rebinding pages, and refuses non-GET requests without a JSON content type, which blocks cross-site forms.
  - The token (item 120) is 32 random bytes (`crypto.getRandomValues`), hex-encoded and kept in memory. Once the port is bound, the server writes it to `$SWITCHBOARD_HOME/token` with mode 0600. On shutdown it deletes the file only if it wrote it, so a second copy that exits on a used port (item 67) leaves the live server's token alone. The file is how the window app gets the token. On Windows the user's profile folder is private to the user by default.
  - The Electron window loads `http://127.0.0.1:<port>/#token=<token>`. A URL fragment is never sent to a server, and the page moves it into localStorage and clears it from the address bar. localStorage is separate per port.
  - `call()` in `index.html` sends the token as an `X-Switchboard-Token` header. `EventSource` and `WebSocket` cannot set headers, so their URLs carry `?token=`, and so do attachment and image URLs. The server's check is a pure helper: header or query, compared in constant time.
  - It is not a cookie, because browsers do not separate cookies by port. A cookie for `127.0.0.1` would also go to every other server there, such as a dev server or a container's published port, and that server could read the token.
  - A WebSocket upgrade is accepted only when `Origin` is `http://127.0.0.1:<port>` or `http://localhost:<port>`, on top of the Host check. Browsers do not apply CORS to WebSockets: any website can open `ws://127.0.0.1:4477`, and its Host header is then correct. The Origin check stops websites; the token stops programs, which can simply leave the Origin header out.
- **Run model (verified in the ACP spike, `designs/acp/acp-spike.md`).** `acp.ts` is the driver; `runner.ts` keeps the delivery, queue, session and attention rules. Each TODO's live session, and the operator's, has one agent process, started on its first run and kept for later turns. It is closed after 30 minutes idle, on remove, on New session and on shutdown, and the next turn starts a new one with `session/resume`. One run is one turn: one `session/prompt` with the delivered messages, whose response (`stopReason`) is the idle signal, so queued messages are sent when it ends. Interrupting is `session/cancel`. A new session's id comes from `session/new`, so the `sessions` row is written after the agent answers; for Claude it is the Claude Code session id. When a turn would change what the session was opened with (an edited project prompt, other folders), the agent process is replaced and the session resumed with the new values. Every `session/new` and `session/resume` is followed by `session/set_config_option mode=auto`, because a session starts in the agent's default mode. The app's own MCP tools are listed in `allowedTools`, so auto mode never asks about them. A permission request a worker still sends (auto mode asks about little) is answered with the reject option and reported like a denial (item 50).
- **Prompt layering (verified in the spike).** The contract and project prompt go in the session's `_meta.systemPrompt = {append: <contract + project prompt>}`, which the Claude adapter applies to the `claude_code` preset. By default Claude Code records the system prompt on a session's first request and reuses it on every resume, so a changed project prompt would never reach a resumed agent. `_meta.claudeCode.options.extraArgs = {'system-prompt-snapshot': 'off'}` renders it fresh on every launch of the agent process. The contract is versioned with the app. The project prompt is user content.
- **Denials (verified in the ACP spike).** A denied tool call (a settings rule or the auto mode classifier) arrives as a `tool_call_update` with `status: failed` and `_meta.claudeCode.toolResponse.decisionReasonType` (`subcommandResults`, `classifier`, …); no permission request is sent. The command comes from the tool call's `rawInput`. The agent receives the denial as an error tool result.
- **Interrupt (verified in the ACP spike).** `session/cancel` during a Bash call kills the command, the agent sees it as a rejected tool use, and `session/prompt` answers `stopReason: cancelled` within milliseconds. The transcript is kept, and the next prompt in the same session works: the agent knows its last command was stopped.
- **Attention is deterministic.** "What needs me" comes from run status, attention flags and denials, not from an LLM. The operator plays no part in signaling. This is deliberate: a Paperclip-style CEO agent watching all sessions would cost usage continuously, could paraphrase or miss things, and would rebuild the long-thread problem this design avoids. Here the user is the CEO, and the operator is a chief of staff that only acts when asked.
- **Sessions (items 68–79).** `todos.session_id` always holds the live session's id, so delivery (item 20) and restore keep resuming `todos.session_id`. `start()` in `runner.ts` is the one place that opens a session: it inserts the `sessions` row, sets `todos.session_id` and runs with `{ sessionId }`.
  - Start uses it for session 1, with the pending messages.
  - `POST /api/todos/:id/new-session` uses it for every later session, after it sets `ended_at` on the previous one and clears attention. The `new session` event is written after `start()` has inserted the new `sessions` row, so it belongs to the new session. Its body and attachments are the same as for a message. It returns 409 while the TODO runs, before session 1, and after removal.
  - `session_no` is set by `take()` when a message is delivered, and from the live session by `addMessage` (agent and app messages), `addEvent` and `attach_file`.
  - `handOver()` in `lib.ts` builds the hand-over header (item 73) as a pure function and leaves out lines with no value. `deliveryPrompt` puts it first on the first turn of every session after the first.
  - The Files fetch walks the artifact folder, skipping `uploads/` and `terminal/`, and registers unknown files with no `session_no`, so they get a file id the editor can open (item 85).
  - `GET /api/todos` summaries add `session_count`. `GET /api/todos/:id` adds `sessions` and `ctx`, and its `files` add `session_no`. Removed rows add the session count.
- **Context usage (item 123).** The agent's `usage_update` gives `used` (the context of its last model call) and `size` (the window); Claude adds the model in `_meta["_claude/model"]` (`ctxStep()` in `lib.ts`). `turns` grows by one per run. The JSON `{used, window, turns, model}` is stored in `todos.ctx`, or under the `meta` key `operator_ctx` for the operator; New session sets it to null. `GET /api/operator` adds `ctx`. `last_in` and `last_out` are no longer reported, so the meter's "last turn" row is left out.
- **Result meta (item 128).** A `result` event's `events.meta` holds JSON `{duration_ms, tools}`: the turn's duration as the app measured it and its number of tool calls. The result text is what the agent wrote after its last tool call. The operator tools line (item 127) needs no new data: it reads the operator's tool events.
- **Backlog (items 86–99).** No new status or table: the backlog is `removed_at IS NULL AND status = 'not_started'`, ordered by `updated_at DESC`, which `addMessage` already bumps on a new note.
  - `POST /api/todos` takes an optional `start: true` and calls `start(id)` after `createTodo`.
  - `POST /api/todos/:id/edit` takes `{ instruction?, project_id? }` and runs in one transaction: check `not_started`, update the first message and the title, and on a project change rename the artifact folder and rewrite `attachments.path`. It writes an `edited` event with actor `user`, and answers 409 once the TODO has started.
  - Summaries add `creator` (the author of the first message, for the `Operator` badge) and `pending_attachments`.
  - The browser keeps the view in localStorage `switchboard-board-view`, and the Events switch in `switchboard-chat-events`. The New TODO card stays in the page and is hidden with the `hidden` attribute on the board, because its input and its paste and drop handlers are looked up once.
- **Files API (items 101–110).** Every request names a root (`worktree` or `project`) and a relative path. `resolveIn(root, rel)` takes the real path, accepts only paths inside the project's local folder or its worktree root (`isInside`), and refuses writes inside `.git/`.
  - `GET /api/todos/:id/tree?root=` returns `{paths, changed, symlinks, base}`. Paths come from `git ls-files --cached --others --exclude-standard -z` plus the root's own symlinks, found with `readdir` and `lstat`. Marks come from `git --no-optional-locks status --porcelain=v2 -z` and `git diff --name-status -z <merge-base>`, with the merge base of that folder's `HEAD` and `origin/HEAD`. Each tree request runs two or three git processes of about 0.03 s each, the same kind of cost as `gitState()`.
  - `GET /api/todos/:id/fs?root=&path=` returns `{content, mtime, size, binary, image, large}`. Binary means a NUL byte in the first 8 KB, large means over 2 MB, and images are recognised by extension.
  - `GET /api/todos/:id/fs/raw?root=&path=` serves image bytes with `Content-Security-Policy: sandbox`.
  - `PUT /api/todos/:id/fs` takes `{root, path, content, base_mtime, force}`, answers 409 `{error: "changed", mtime}` on a conflict, like the TODO file `PUT`, and writes the `file saved` event.
  - `GET /api/todos/:id/diff?root=&path=` returns `{base, base_sha, current}`, with the base read by `git show <merge-base>:<path>`. The browser computes the line diff with `diff()` from `@codemirror/merge`, over lines mapped to characters.
- **Terminals (items 111–119).** A PTY in the Bun server: `Bun.spawn(cmd, { cwd, env, terminal: { cols, rows, data, exit } })`, then `proc.terminal.write()` and `resize()`. The exit code comes from `proc.exited`; `terminal.exit` reports the PTY's status, not the process's. node-pty is not an option: under Bun its PTY closes after the first output (tested with Bun 1.4.2), and putting it in Electron would leave browser mode without terminals.
  - A Claude terminal runs `["claude", "--add-dir", <artifact folder>, <message>]` directly, not through a shell, so the first message needs no quoting.
  - Terminals live in the server's memory, not in the database. Each keeps its kind, root, folder, number (counting up per kind), title (parsed from OSC 0 and 2 escape sequences by `parseOscTitle()`) and the last 256 KB of output.
  - `GET /api/todos/:id/terminals` lists `[{id, kind, root, cwd, no, title, exited, code, signal}]`. `POST /api/todos/:id/terminals` takes `{kind: "shell" | "claude", root: "worktree" | "project", message}` and returns `{id}`; for Claude it writes the context file first. `POST /api/todos/:id/terminals/:tid/restart` restarts in the same tab. `POST /api/todos/:id/terminals/:tid/close` ends one. Opening, closing, exits and title changes reach open pages as an SSE `{type: "terminals", id}` event.
  - One WebSocket per open terminal: `GET /api/todos/:id/terminals/:tid/ws`, upgraded with `server.upgrade()` and the `websocket` handler of `Bun.serve`. Binary frames carry terminal bytes both ways. Text frames carry JSON: `{type: "resize", cols, rows}` from the browser, `{type: "exit", code}` from the server.
  - On attach the server sends a terminal reset, then the buffered output, then live output, and re-sends the size so full-screen programs (vim, htop, Claude Code) redraw. If replaying the raw buffer garbles too often, the fallback is Orca's approach: mirror each terminal into `@xterm/headless` on the server and send `addon-serialize` snapshots.
  - `contextFile()` in `lib.ts` builds `terminal/context.md` (item 114) as a pure function.
  - At start the server checks `Bun.version` (1.3.5 or newer) and the platform (item 116).
- **Theming.** Tokens and components follow the design: `--accent-magenta`, `--tint-success` and `--tint-magenta`, a Switch (the Events switch), the toast action, and icon buttons that are plain by default with an outlined variant.
  - CodeMirror's theme uses the CSS variables directly, so Auto, Dark and Light need no extra code: keyword violet, string amber, number magenta, comment muted, property cyan, a lime caret and `--tint-lime` selection; in markdown, headings cyan, quotes violet, code amber and links lime.
  - xterm.js needs colour values instead. They are read with `getComputedStyle` when a terminal opens and again in `applyTheme()`: foreground `--text-primary`, background `--surface-inset`, red `--accent-red`, green `--status-success`, yellow `--accent-amber`, blue `--accent-blue`, magenta `--accent-violet`, cyan `--accent-cyan`, bright black `--text-muted`, selection `--tint-lime`. The cursor is a steady lime block.
  - The terminal opens after `document.fonts.ready`, so it measures JetBrains Mono rather than the fallback font.
- **Window app (item 121).** On macOS the window uses `titleBarStyle: 'hidden'` with `trafficLightPosition: {x: 14, y: 14}`. The `enter-full-screen` and `leave-full-screen` events toggle a `fullscreen` class through `webContents.executeJavaScript`, which drops the spacer. There is no preload script. The window has `minWidth: 960`. The menu comes from `Menu.buildFromTemplate` with the `editMenu`, `viewMenu` and `windowMenu` roles; without `editMenu`, ⌘C and ⌘V stop working on macOS. After `waitForServer()` the window reads `$SWITCHBOARD_HOME/token` (default `~/.switchboard/token`) and loads the URL with `#token=`.
- **Windows and Linux.** Only macOS is verified. The branches that cost a line each stay in the code, marked with `ponytail:` comments: `SWITCHBOARD_SHELL`, the Ctrl key rules, and skipping `loginShellEnv()` in `electron/main.js` on Windows. bun-types 1.4.2 marks the `terminal` option of `Bun.spawn` as POSIX-only, so Windows gets no terminals for now (item 116). `install-local` is macOS-only.
- **Data.** The schema is the `CREATE TABLE` block at the top of `server.ts`. Not visible there: a message's `state` is `pending` (stored until the start), `queued` or `delivered`; drafts are keyed `todo:<id>` or `operator`; the operator's messages have `todo_id = NULL`, and a fresh operator session hides older ones through the `operator_since` value in `meta`. Sessions live in `sessions (id, todo_id, no, session_id, started_at, ended_at)`, unique on `(todo_id, no)`. `messages`, `events` and `files` have a nullable `session_no`. `todos.ctx` and `meta.operator_ctx` hold the context usage, and `events.meta` the result footer.
- **Migration.** New tables and columns follow the existing pattern in `server.ts`: check `pragma_table_info`, then `ALTER TABLE ADD COLUMN`. Every change only adds, so older code still reads the database. On startup, every TODO with a `session_id` and no `sessions` row gets session 1, with `started_at` from its first `launch` event (else `created_at`); its delivered messages, its events and its files get `session_no = 1`.
- **Data folder.** `~/.switchboard/` holds `db.sqlite`, `token` while a server runs, `data/<project-slug>/<todo-id>/` for artifact files (with `uploads/` for attachments and `terminal/context.md` for Claude terminals), and `old-review-prompt.md` from the migration.

## Not built on purpose

- Native or macOS notifications. The tab title count and the amber squares are the only signals.
- Cleaning up worktrees, branches or sessions. The user does this manually, or as a TODO using the git-tidy skill.
- Recreating a missing worktree or session on restore.
- Concurrency caps, queueing of starts, and usage-limit handling. Agents that hit the limit go idle, and the user checks usage in Claude.
- Guarding against two TODOs sharing a worktree or branch, or against a TODO's agent and its terminals running git at the same moment. Overlap is sometimes intentional; the branch on each card and the warning in a Claude terminal's context file (item 114) are the only aids.
- Pausing a running TODO, or moving a started TODO back to the backlog.
- Routing permission prompts to the UI.
- Operator autonomy: answering workers, starting TODOs, or acting without an instruction. The operator also has no tools to edit, reorder or start backlog TODOs.
- Projects spanning multiple repositories.
- A dispatcher or orchestrator pass.
- Restricting worker agents beyond the user's normal Claude Code config.
- Deleting projects.
- A model setting.
- Changing `cleanupPeriodDays`. Raising it is the user's choice in their own settings.
- Parallel app sessions in one TODO. Use a second TODO, or a terminal (item 83).
- Messaging, resuming or restarting an earlier session from the app, or opening one of the TODO's sessions in a terminal. Item 78 covers resuming one outside the app. An app run cannot be shown in a terminal anyway, because the ACP adapter runs Claude Code without its terminal UI.
- Removing a single session, or moving sessions between TODOs.
- Hand-over notes or summaries written automatically by an agent (item 74).
- Starting a new session automatically, for example when the context fills up. The context meter (item 123) only advises.
- Tracking `claude` processes that run in terminals. Their MCP tools could now come from `/mcp/app` with a secret of their own (see `designs/acp/acp-design.md` §10). If it is ever wanted, a `kind` column on `sessions` is where it would go.
- Manual ordering, priorities, tags, due dates, or a task or any other grouping level above the TODO. If recently-touched turns out not to be enough, a rank column with drag-and-drop is the next step.
- Text search in the backlog. The project filter is the only narrowing.
- Editing or removing attachments or notes after they are committed.
- Starting several backlog TODOs at once.
- Refreshing a running Claude terminal's context. Its context is a snapshot from launch, and Claude can re-read the TODO's files and git state itself.
- Split panes.
- Files and terminals for a project without a TODO, for example in Settings › Projects.
- Terminals that survive quitting the app (a separate daemon as in Orca, or tmux as in Kanban Code). Quitting stops terminals, as it stops agents.
- Creating, renaming and deleting files; search across files; LSP; git actions (stage, commit, discard) from the UI.
- Showing ignored files, apart from the root's own symlinks (item 102).
- A browser preview pane (Orca and Kanban Code have one).
- Telling the agent about file edits or terminal commands. The user says it in chat.
- The design's "Show flow" panel and launch command preview (item 60).
- The design's other "Open questions" options (`sortBelow`, `countUnreadIdle`, `mergedCue`, `prBehindTone`).
- On Windows and Linux: drawn window controls or `titleBarOverlay`, terminals on Windows (`pwsh` detection, running `claude` `.cmd` shims through `cmd /c`), and packaging (electron-builder `nsis` and `AppImage` targets).

## Spike findings (2026-10-02)

Throwaway script run against a test repository, on `claude-sonnet-5-5` with the user's full Claude Code config loaded. These findings are about the Agent SDK `query()`, which the app used before ACP; `getSessionInfo` and the hooks are no longer used. The ACP spike is in `designs/acp/acp-spike.md`.

| Question | Result |
|---|---|
| Does a changed appended prompt reach a resumed session? | With `extraArgs: {'system-prompt-snapshot': 'off'}`: yes. Turn 1 had codeword ALPHA, the resume had BRAVO, and the agent answered BRAVO. Without the flag it answered ALPHA. |
| Does `resume` keep the session id? | Yes, in all four sessions. Passing `sessionId` on the first turn works. |
| What does `interrupt()` mid tool call leave behind? | The Bash command was killed (no process left). The loop throws with `error_during_execution`. `Stop` does not fire. Resume works. Partially written files were not tested. |
| Does `getSessionInfo` detect a missing session? | Yes: it returns `undefined` for an unknown id. |
| Is the agent told when a tool call is denied? | Yes: the tool result reads "Permission to use Bash with command … has been denied." The agent said it would not retry or work around it. |
| Does the `PermissionDenied` hook fire? | No. The SDK message `system/permission_denied` and `result.permission_denials` both reported the denial. |
| Does the worktree skill run under auto mode? | Yes, with no denials. The agent called `set_worktree` with the correct absolute path, and its `cd` persisted across commands. The `CwdChanged` hook never fired. |
| Does the user's config load into workers? | Yes: the user's hooks changed the agent's reply style, its final text ended with the user's three report headings, and all of the user's MCP servers were loaded (several were still pending or needed auth). |

Checked 2026-10-06, for item 78: does `claude --resume <id>`, run from the project's local folder, pick up a session the Agent SDK created? Yes, by transcript location. An app session's transcript is at `~/.claude/projects/<cwd-slug>/<session-id>.jsonl`, the same store the `claude` CLI reads. The file was found there; `--resume` itself was not run.

Spike 2026-10-06, for item 73: does the agent of a later session read the handed-over files on its own, or only when the instruction says so? Session 1 wrote `plan.md`. New session with "Continue the work: add a fourth color to the list": session 2's first tool call was Read of `plan.md` (from the hand-over file list), then Edit. The instruction named the list, not the file. The header alone is enough; the contract rule (item 31) stays "read the listed files you need".

## Open questions

- Answered by the ACP spike: the classifier does deny (`curl … | sudo bash`, `decisionReasonType: classifier`), though not every time. Earlier question: does the auto-mode classifier itself deny anything under SDK `query()`? The one denial seen had reason `subcommandResults` (a compound command), not `classifier`. A single `git push --force origin main` and `curl … | sh` were both allowed, and an exfiltration-style command was refused by the model before any tool call. The design does not depend on the answer, because item 50 reacts to any denial.
- In one spike run, the agent passed the parent `worktrees/` folder to the worktree skill's `copy-local-settings.sh`, so `.env` was linked next to the worktree instead of inside it, and the agent then reported success. The user's real worktrees have their `.env` links in the right place. Watch for this in practice; the fix would be one word in the skill's step 5.

## Reference

How others build files, editing and terminals around coding agents. Researched 2026-10-06, from each project's source.

| | Shell and UI | Terminal | Files and editing | Git |
|---|---|---|---|---|
| [Warp](https://github.com/warpdotdev/warp) | Rust, its own GPU UI framework (Metal on macOS, wgpu on Windows and Linux) | Its own emulator adapted from alacritty_terminal, with a forked ConPTY on Windows | Its own Rust editor, tree-sitter, LSP | git2 |
| [Orca](https://github.com/stablyai/orca) | Electron, React 19, Tailwind, shadcn/ui | xterm.js 6 and node-pty in a detached daemon; @xterm/headless for reattach | A custom virtualized tree, @parcel/watcher, ripgrep, Monaco and its diff editor, no LSP | The `git` and `gh` CLIs |
| [Kanban Code](https://github.com/langwatch/kanban-code) | Native Swift on macOS, plus a separate Tauri and React port for Windows | SwiftTerm attached to tmux; on Windows xterm.js with tauri-pty | No tree or editor; "Open in Zed/Cursor/VS Code" | The `git` and `gh` CLIs |
| [Vibe Kanban](https://github.com/BloopAI/vibe-kanban) | A local Rust server, React in the browser, an optional Tauri wrapper | xterm.js with portable-pty for the user's shell; agents as stream-json processes | A diff viewer and a tree of changed files, no editor | The `git` CLI and libgit2 |

Switchboard sits closest to Vibe Kanban in shape: a local server, a browser UI, a thin desktop wrapper, and agents driven through a structured stream instead of a terminal. It takes its file and terminal features from Orca, using CodeMirror instead of Monaco and Bun's PTY instead of node-pty.
