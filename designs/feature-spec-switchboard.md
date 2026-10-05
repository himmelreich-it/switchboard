# Switchboard

Switchboard is a local app, in the browser or in its own desktop window, where every TODO is a Claude Code session. The user creates TODOs across multiple projects on one page, starts each one when ready, steers it with short one-line chat messages, and stops or restores it by removing or restoring the TODO. Anything substantial an agent produces (plans, reviews, questions with context, designs) goes into markdown files the user can open and edit inline, so the chat stays small. A pinned operator session answers questions about all sessions and acts only when told. A PR review is a TODO like any other. Guiding principle: **streamline the user's work, don't delegate their control.**

**This is an organizer, not an auto-builder.** Worker agents run exactly as the user's normal Claude Code sessions do: same user, project and local settings, same global `CLAUDE.md`, skills, plugins, hooks and MCP servers. The app adds a way to see, start, steer and stop them. It does not add restrictions, tool allowlists or safety layers beyond what the user's normal sessions have.

This document describes Switchboard's functionality and how it is built, and is the guideline for building it. Each numbered item is one piece of functionality. The numbers are stable, because the designs and code comments cite them (for example `R25` in `index.html`). Items are grouped by area, not by number; a new item takes the next free number.

## Design

The UI always follows the design. The current design is `designs/Switchboard-v3.html`; `designs/Switchboard-v1.html` and `designs/Switchboard-v2.html` are earlier versions, kept for reference. Where the UI and the design differ, the UI is changed to match the design. The one exception is item 60: the design's "Show flow" panel and launch command preview are not built.

UI changes are checked against the running app on localhost, in Chrome or in the window app.

## Functionality

### Running the app

64. `bun server.ts` serves the app on `http://127.0.0.1:4477`. `PORT` changes the port and `SWITCHBOARD_HOME` the data folder (default `~/.switchboard`).
65. `bun run app` opens the app in an Electron window (macOS, Windows, Linux) that starts `bun server.ts`. When the window app quits or crashes, the server's stdin closes and the server shuts down as in item 56. Links in rendered markdown open in the default browser. If a server already runs on the port, the window attaches to it.
66. `bun run install-local` builds `Switchboard.app` (no installer) and copies it to `/Applications`, so it opens from Spotlight and stays in the Dock. It quits a running Switchboard first, which stops its agents. The installed app runs a copy of the code (run `install-local` again to update it) and still needs `bun`. It loads the login shell's environment (`zsh -ilc`), because apps started from the Dock get a bare environment, so `bun`, hooks and MCP servers find their PATH and keys.
67. A second copy started on a port that is in use exits without touching any TODO: the startup reset of item 56 runs only after the port is bound.

### Projects

1. The Projects page creates and edits projects, each with a name, a local folder path (`~` is expanded) and a free-text agent prompt. The repository is not typed in: the app reads it with `git -C <local_path> remote get-url origin` and shows it read-only. Saving a project whose local folder is not a git repository fails with an error. The page lists each project with its repository and TODO count.
2. A project maps to exactly one repository.
3. Agents for a project launch with the project's local folder as their working directory and with all setting sources loaded (user, project, local), so the user's global `CLAUDE.md`, skills, plugins, hooks and MCP servers apply, as well as the repo's `CLAUDE.md`, `CLAUDE.local.md` and `.claude/` configuration.
4. The project's agent prompt is delivered to every agent launched for that project, including resumes, appended to the Claude Code system prompt after the app contract (item 31). Each launch sends the current contract and project prompt, so a resumed agent sees the current versions, not the ones from its first turn (see prompt layering under How it is built). Typical content: instructions to use the user's worktree skill, which also symlinks untracked local files (e.g. `CLAUDE.local.md`, `.env`) into new worktrees.
43. Each project gets a slug when it is created. The slug never changes, even when the project is renamed, because the project's artifact folder is named after it (item 22).
44. A project's local folder becomes read-only once any TODO in that project has started, because Claude Code stores session transcripts under the folder the session was launched from, and a changed path would make every started TODO impossible to resume.
45. Projects cannot be deleted.

### TODOs and sessions

5. One page lists all TODOs across all projects as cards. A card shows the status with Queued, Draft and Unread badges, the title, the project, and the branch when a worktree is registered (item 46). The branch is read with `git --no-optional-locks status --porcelain=v2 --branch` in the worktree. The card also shows the last message (an `app` message wins over the last agent message) and a footer with the last tool, the messages waiting for the start, or the file names.
   - A TODO with a worktree shows a git line: commits on the branch, changed and untracked files, push state. The Focus view also shows how far the branch is behind its base. Git state is read on every list fetch.
   - A TODO with a worktree, or with a PR registered via `set_pr`, shows a PR line that links to GitHub: state, draft, conflicts, checks, review decision, open threads, behind base. PR state comes from one `gh api graphql` call per project every 2 minutes and after each run, run as whichever logged-in `gh` account can read the repository, without switching the active account.
   - When `gh` fails, the last PR result stays, greyed. The Focus view shows the reason with a Retry button, and the header shows a "PR status" button.
6. Adding a TODO takes a project, a first instruction and optional attachments (item 62); ⌘↵ adds it. The project defaults to the one picked in the project filter (item 63), else to the most recently chosen one. A new TODO is `not started yet` and no agent is launched.
7. A TODO's initial title is its first instruction. Once the agent runs, it replaces the title with a generated one via `set_title`.
8. The user starts each TODO explicitly, with Start on its card or in the Focus view. Starting launches one Claude Code session with the first instruction and every message committed before the start, as one prompt.
9. Each started TODO is exactly one Claude Code session, and its session id is stored on the TODO. The app generates the session id and passes it on the first turn, so the id is known before the agent runs.
10. A TODO's status is `not started yet`, `running`, `waiting on you` or `idle`. `running` and `idle` follow the run lifecycle (item 30). `waiting on you` lasts while the attention flag is raised. The agent clears its own flag; the user can also clear it with Clear attention. Each raise and clear is recorded with who did it (`user`, `agent` or `app`). A TODO has no "ended" state: `idle` is its resting state, and removing it is how it ends.
11. There is no pause action for a started TODO. `not started yet` exists only before the first start.
12. A TODO can be removed in any status; the app asks for confirmation only while the agent runs. Removing interrupts a running agent, like pressing Esc in Claude Code; if the run has not ended 10 seconds later, the process is killed. The TODO gets `removed_at` and `removed_by` and disappears from the main list together with its messages and files. Messages still queued at removal stay queued.
13. The Removed view lists removed TODOs, newest removal first, with their project, who removed them and when, session id, message and file counts, branch, a Draft badge and a Restore button.
14. Restoring a removed TODO clears its removal, brings back its messages and files, and resumes its session by session id. The prompt is the messages still queued at removal plus the user's uncommitted draft, sent as one prompt; with neither it is "continue". The "continue" is queued like any message, so a restore while the interrupted run is still ending goes out when that run ends. A TODO removed before it was started goes back to `not started yet` with its pending messages, and nothing launches.
15. If the session, the worktree or the project's local folder of a TODO is gone, restore shows a warning and does nothing else: it does not recreate the worktree or start a new session. The session check is `getSessionInfo(session_id)` returning nothing; the worktree and folder checks are the registered worktree path or the project's local folder not existing on disk. The warning mentions that Claude Code deletes session transcripts after 30 days by default (`cleanupPeriodDays` in the user's `~/.claude/settings.json`).
16. A PR review is an ordinary TODO, e.g. "review PR #482" or a ticket reference.
46. When an agent creates a worktree, it registers the worktree's absolute path via `set_worktree`, and the app stores it on the TODO. While a worktree is registered, every later prompt starts with the line `Your worktree: <path>. Work there.`, because each run starts in the project's local folder, not in the worktree.
47. Every launch adds two folders to the session's additional directories: the project's worktree root, so the agent can work in the worktree and its `cd` persists across commands, and the TODO's artifact folder, which is outside the working directory and where the agent writes its files. The worktree root is `git -C <local_path> config worktree.root` if set, otherwise `<local_path>/../worktrees`; it is created if missing.

### Chat

17. Each TODO has a chat with the user's messages and the agent's messages.
18. Agent chat messages are one or two lines at most. Longer content goes in a markdown file, and the message links to that file. `post_message` rejects longer messages, and links only a file already registered with `attach_file`.
19. The message composer follows a type, then commit flow (⌘↵). Nothing reaches the agent until the user commits. An uncommitted draft survives navigating away and reloading the page, and is shown with a Draft marker. Drafts are stored on the server, because restore uses them (item 14); the browser saves them 400 ms after the last keystroke and when the page is hidden.
20. A committed message is delivered according to the session state, and the composer's hint says which case applies:
    - When the agent is `running`, the message is queued and delivered once, when the run ends, matching Claude Code's default behavior.
    - When the agent is `idle` or `waiting on you`, the message is delivered at once by resuming the session.
    - When the TODO is `not started yet`, the message is stored and delivered at start.
    - Several messages waiting for the same delivery are sent as one prompt.
21. All messages are stored permanently with author and timestamp. Authors are `user`, `agent`, `user via operator`, `operator` (the operator's replies in its own chat) and `app` (messages the app itself writes into a TODO's chat, items 48, 49, 50). The chat marks queued messages, messages waiting for the start, and messages sent via the operator.
48. If a turn ends and the agent posted no message in it, the app shows the turn's final result text as the agent's message, cut to two lines and 280 characters. The full text is kept in the TODO's events.
49. If a run fails (the Claude Code process exits with an error, for example a usage limit), the app posts an `app` message naming the error, e.g. `claude exited 1: usage limit`, and the TODO goes to `idle` (or `waiting on you` while its attention flag is raised). A run the app interrupted itself (removal, shutdown) produces no error message.
50. When the permission system denies a tool call, the app posts an `app` message such as `Auto mode blocked Bash: git push --force origin main` and raises the TODO's attention flag with actor `app`. The agent may clear the flag after the user's reply, as with any other attention.
51. A `running` TODO shows its most recent tool on its card and in the Focus view, e.g. `PreToolUse · Bash`.
62. The user can attach images and other files to a TODO's first instruction and to later messages, by pasting, dropping or picking them. A follow-up may be attachments only.
    - Each file is saved to `<artifact folder>/uploads/<message id>-<n>-<name>`, and the prompt lists the absolute paths after the message text, so the agent opens them with `Read`.
    - Files waiting to be sent show as cards with a preview, their size and a × to drop them. They are kept only in the browser tab until the message is committed.
    - The chat shows sent images as thumbnails and other files as download links with their size. Attachments are served with `Content-Security-Policy: sandbox`, so an uploaded SVG or HTML file cannot run script.
    - A pasted image with the browser's generic name (`image.png`) is renamed `pasted-<HHMM>.png`. When the clipboard holds text and an image (a table copied from a document), the text pastes and the image is attached.
    - Files are sent as base64 inside the JSON body, because the server refuses non-JSON POSTs.
    - The operator chat takes no attachments.

### Markdown files

22. Agents write plans, reviews, questions with context and designs as markdown files in the TODO's artifact folder, `~/.switchboard/data/<project-slug>/<todo-id>/`. The folder's absolute path is given in the app contract, so files survive worktree cleanup and stay out of git.
23. Agents register each file they create against their TODO via `attach_file`, which resolves symlinks and rejects any path outside the TODO's artifact folder. The Files tab lists each file with its path and line count, or "missing on disk". A chat message that links a file shows a chip that opens it.
24. The user can open a file in the app, edit it inline, save and close it. Saving does not notify the agent. The user hands work back with a chat message such as "updated the review, post it but drop point 3".
25. A file is read-only while its TODO's agent is `running`, and editable in any other status. If the TODO starts running while the user has unsaved edits (for example an operator message or a queued delivery resumes it), the edits stay in the browser and the editor locks. Saving is allowed again once the TODO stops running. If the agent changed the file on disk in the meantime, saving warns that the file changed and offers Overwrite or Discard. An open file without unsaved edits reloads when the file changes on disk.
26. Files are stored on disk. The database stores only the mapping between files and TODOs.
52. The file editor shows a plain text editor and a rendered markdown preview side by side. The read view shows the rendered markdown only.

### Agent launch and app contract

27. Agents run in auto permission mode (`permissionMode: 'auto'`).
28. Hooks and the app's MCP server are passed per run as options of the Agent SDK `query()` call (in-process hook callbacks and an in-process MCP server), not via the repository's settings, so only app-launched agents report to the app and the user's normal Claude Code sessions in the same repo are unaffected.
29. Worker agents have these MCP tools: `post_message`, `set_title`, `set_attention` (raise and clear), `attach_file`, `set_worktree`, and `set_pr` (the pull request the TODO works on, also without a worktree; otherwise the PR is found by the worktree's branch). The contract names them by their full tool ids (`mcp__app__post_message` etc.), because with the user's many MCP servers the agent loads tool definitions on demand.
30. Status follows the run lifecycle:

    | Event | Status |
    |---|---|
    | A `query()` run starts (start, resume, queued delivery) | `running` |
    | The run ends, normally, with an error, or by interrupt | `idle`, or `waiting on you` if the attention flag is raised |

    The `UserPromptSubmit` and `PreToolUse` hooks only record events and the most recent tool (item 51). The `Stop` hook is not used to set `idle`, because it does not fire when a run is interrupted. `SessionEnd` does not change status.
31. Every agent receives a fixed app contract that the user cannot edit, appended to the system prompt before the project prompt. The Projects page shows it read-only. The contract instructs the agent to:
    - talk to the user only through the app's MCP tools, and set a title on its first turn;
    - keep chat messages to one or two lines and put anything longer in a file under the given artifact folder, linked from the message;
    - treat the TODO text and the user's committed messages as its go-ahead for local edits within this TODO's scope, while outward actions (push, PR comments, Jira, Slack) still need an explicit message from the user;
    - work out from the TODO text, and from any referenced ticket, what kind of work this is (a review, a feature, or thinking only with no code changes) and whether it needs a worktree, following the project prompt for how to create one;
    - after creating a worktree, call `set_worktree` with its absolute path and work only there, ignoring any skill instruction not to switch context;
    - call `set_pr` as soon as it knows the TODO works on an existing GitHub pull request, also without a worktree;
    - ask, via a message plus the attention flag, rather than guess when anything is unclear;
    - post a message and raise attention when auto mode blocks an action or it is unsure how to proceed, instead of routing around the block;
    - clear its own attention flag once the user's reply resolves it.
32. There is no dispatcher or orchestrator pass. A started agent interprets its own TODO.
53. The app has no model setting. Agents use the model from the user's Claude Code settings.

### Operator

33. A pinned operator card sits above the TODO lanes. It shows the operator's status and its last two messages, with replies rendered as markdown. Its input sends on Enter (Shift-Enter adds a line). Opening the card shows the operator in the same chat UI as a TODO, where ⌘↵ commits.
34. The operator has these tools: `list_sessions` (titles, projects, statuses, attention, branch, git and PR state, files and latest messages, optionally removed TODOs), `read_session` (a TODO's details, messages and file list, removed TODOs too), `read_file` (a TODO's markdown file by name), `create_todo` and `send_message`, plus Claude Code's Read, Grep, Glob, WebSearch and WebFetch. It has no Edit, Write, Bash, none of the user's MCP servers, and no tool to start TODOs; anything else is refused (`permissionMode: 'dontAsk'`). It runs with `~/.switchboard` as its working directory.
35. The operator acts only on the user's explicit instruction. It never polls, never answers a worker's question on its own, and never starts TODOs.
36. TODOs created by the operator start as `not started yet`. The user starts them.
37. A message the operator sends on the user's behalf appears in the target TODO's chat as a user message with a "via operator" marker, and is delivered like a message the user commits (item 20).
38. The operator gets session state through its tools rather than from its conversation, so the user can start a fresh operator session at any time with New session without losing anything. A fresh session gets a new session id and hides older operator messages.
39. Brainstorming, such as thinking through a future feature, works in the operator or in a TODO the agent treats as thinking only.
54. Operator replies are shown in full, rendered as markdown. The one-or-two-line rule (item 18) does not apply to the operator.

### Attention

40. The main list sorts `waiting on you` TODOs to the top. Below them, TODOs are grouped in lanes in this order: `running`, `not started yet`, `idle`. Within each lane, the most recently updated TODO comes first.
41. The browser tab title shows the number of TODOs in `waiting on you`, e.g. `(2) switchboard`. Only `waiting on you` counts.
55. A TODO shows an Unread badge when an agent message arrived after the user last opened that TODO; opening it clears the badge. Unread TODOs are not counted in the tab title and do not move to the waiting lane.

### Persistence

42. A local SQLite database stores projects, TODOs and their sessions, messages, drafts, attachments, file mappings, events (attention changes, runs, hooks, denials) and removals (`removed_by`, `removed_at`). This history stays available for the operator after a TODO is removed.
56. On shutdown (Ctrl-C, SIGTERM, or quitting the window app), the app interrupts all running agents and waits up to 5 seconds; a second Ctrl-C exits at once. On startup, any TODO still marked `running` is set to `idle` (or `waiting on you` while its attention flag is raised) with an `app restarted` event. Messages still queued stay queued. A Deliver button shows whenever queued messages wait and the agent is not running. Nothing is delivered automatically on startup.
57. On first start (no database yet), the app imports the projects (name, local path) from `~/.auto-coder/db.sqlite`, leaves the old reviews and findings behind, and writes the old stored review prompt to `~/.switchboard/old-review-prompt.md`, so the user can paste it into project prompts.

### UI

58. The screens: the overview with the New TODO card, the pinned operator card, the project filter row and the status lanes; the Focus view of a TODO with Chat, Files and Events tabs, its "1 of 2 waiting" counter and the next waiting TODO; the Removed view; the Projects page; and the Auto/Dark/Light theme buttons (Auto follows the OS setting; the pick is kept in localStorage). The header shows the counts of TODOs, removed TODOs and projects. The Events tab lists events newest first, with their actor.
59. Keyboard shortcuts: `N` focuses the New TODO input, `⌘J` opens the next TODO waiting on you, `⌘E` toggles file edit mode, `⌘S` saves the open file, `⌘↵` commits the draft (or adds the TODO in the New TODO card), `Esc` closes the open file or goes back to the overview. Enter or Space opens a focused card.
60. The design's "Show flow" panel (scenarios, lifecycle trace, live event feed) and the launch command preview on the Projects page are not built.
61. The UI gets live updates from the backend over one SSE stream. After a reconnect it refetches what is on screen.
63. The overview has a project filter row above the lanes: an All chip and one chip per project, each with its count of TODOs and, for a project with TODOs waiting on you, an amber square. Picking a project shows only its TODOs in the lanes, limits `⌘J` and the Focus "1 of 2 waiting" counter to that project, and preselects it in the New TODO card. While a project is picked, the row says how many TODOs (and how many of them waiting) are in other projects, with a Show all button. The tab title and the header counts still count all projects. The pick is kept in the browser's localStorage.

## How it is built

- **Shape.** One local Bun process (`server.ts`) owns the agent runs (`runner.ts`), the SQLite database (`bun:sqlite`) and an SSE stream for the browser UI. Pure helpers live in `lib.ts`, covered by `bun test`. The frontend is a single `index.html` with no build step, using `marked` and `dompurify` from cdnjs. The Electron window (`electron/main.js`) only wraps the unchanged Bun server, because Electron runs Node and the server needs Bun.
- **No login, loopback only.** The server binds to 127.0.0.1. It accepts only requests whose Host header names this server (`127.0.0.1:<port>` or `localhost:<port>`), which blocks DNS-rebinding pages, and refuses non-GET requests without a JSON content type, which blocks cross-site forms.
- **Run model (verified in the spike).** One Agent SDK `query()` call per turn, with `resume: <session_id>` after the first. The run ending (the `for await` loop finishing or throwing) is the idle signal, so queued messages are sent when it ends, and interrupting a run means calling `interrupt()` on its query. Hooks are in-process callbacks passed in `options.hooks`. The app's MCP server is created with `createSdkMcpServer` and passed in `options.mcpServers`. No HTTP hook endpoint, no MCP HTTP server, no temporary settings files. Each callback and tool handler already knows its TODO, so nothing has to be matched by `session_id`. The app's own MCP tools are listed in `allowedTools`, so auto mode never asks about them.
- **Prompt layering (verified in the spike).** The contract and project prompt go in `systemPrompt: { type: 'preset', preset: 'claude_code', append: <contract + project prompt> }`. The SDK has no `appendSystemPrompt` option; passing one is silently ignored. By default Claude Code records the system prompt on a session's first request and reuses it on every resume, so a changed project prompt would never reach a resumed agent. Passing `extraArgs: { 'system-prompt-snapshot': 'off' }` renders it fresh on every launch. The contract is versioned with the app. The project prompt is user content.
- **Denials (verified in the spike).** A denied tool call arrives as an SDK message `{type: 'system', subtype: 'permission_denied', tool_name, message}` and in `result.permission_denials`, and the agent receives the denial as an error tool result. The `PermissionDenied` hook did not fire, so item 50 uses the SDK message.
- **Interrupt (verified in the spike).** `interrupt()` during a Bash call kills the command, gives the agent the tool result "The user doesn't want to proceed with this tool use…", and makes the `for await` loop throw with result subtype `error_during_execution`. `Stop` does not fire. The transcript is kept, and resuming the same session id works: the agent knows its last command was stopped.
- **Attention is deterministic.** "What needs me" comes from run status, attention flags and denials, not from an LLM. The operator plays no part in signaling. This is deliberate: a Paperclip-style CEO agent watching all sessions would cost usage continuously, could paraphrase or miss things, and would rebuild the long-thread problem this design avoids. Here the user is the CEO, and the operator is a chief of staff that only acts when asked.
- **Data.** The schema is the `CREATE TABLE` block at the top of `server.ts`. Not visible there: a message's `state` is `pending` (stored until the start), `queued` or `delivered`; drafts are keyed `todo:<id>` or `operator`; the operator's messages have `todo_id = NULL`, and a fresh operator session hides older ones through the `operator_since` value in `meta`.
- **Data folder.** `~/.switchboard/` holds `db.sqlite`, `data/<project-slug>/<todo-id>/` for artifact files (with `uploads/` for attachments), and `old-review-prompt.md` from the migration.

## Not built on purpose

- Native or macOS notifications. The tab title count is the only signal.
- Cleaning up worktrees, branches or sessions. The user does this manually, or as a TODO using the git-tidy skill.
- Recreating a missing worktree or session on restore.
- Concurrency caps, queueing of starts, and usage-limit handling. Agents that hit the limit go idle, and the user checks usage in Claude.
- Guarding against two TODOs sharing a worktree or branch. Overlap is sometimes intentional; showing the branch on each card is the only aid.
- Pausing a running TODO.
- Routing permission prompts to the UI.
- Operator autonomy: answering workers, starting TODOs, or acting without an instruction.
- Projects spanning multiple repositories.
- A dispatcher or orchestrator pass.
- Restricting worker agents beyond the user's normal Claude Code config.
- Deleting projects.
- A model setting.
- Changing `cleanupPeriodDays`. Raising it is the user's choice in their own settings.

## Spike findings (2026-10-02)

Throwaway script run against a test repository, on `claude-sonnet-5-5` with the user's full Claude Code config loaded.

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

## Open questions

- Does the auto-mode classifier itself deny anything under SDK `query()`? The one denial seen had reason `subcommandResults` (a compound command), not `classifier`. A single `git push --force origin main` and `curl … | sh` were both allowed, and an exfiltration-style command was refused by the model before any tool call. The design does not depend on the answer, because item 50 reacts to any denial.
- In one spike run, the agent passed the parent `worktrees/` folder to the worktree skill's `copy-local-settings.sh`, so `.env` was linked next to the worktree instead of inside it, and the agent then reported success. The user's real worktrees have their `.env` links in the right place. Watch for this in practice; the fix would be one word in the skill's step 5.
