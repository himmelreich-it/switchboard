# Feature: Switchboard TODO Sessions

> **auto-coder became agent-coordinator, then switchboard.** The existing auto-coder project was renamed and reworked into the app described below.

## Overview

Switchboard is a local web app where every TODO is a Claude Code session. The user creates TODOs across multiple projects on one page, starts each one when ready, steers it with short one-line chat messages, and stops or restores it by removing or restoring the TODO. Anything substantial an agent produces (plans, reviews, questions with context, designs) goes into markdown files the user can open and edit inline, so the chat stays small. A pinned operator session answers questions about all sessions and acts only when told. The existing review functionality in auto-coder is reworked into this model: a review is just a TODO. Guiding principle: **streamline the user's work, don't delegate their control.**

**This is an organizer, not an auto-builder.** Worker agents run exactly as the user's normal Claude Code sessions do: same user, project and local settings, same global `CLAUDE.md`, skills, plugins, hooks and MCP servers. The app adds a way to see, start, steer and stop them. It does not add restrictions, tool allowlists or safety layers beyond what the user's normal sessions have.

Requirement numbers 1–42 match the references in `designs/Switchboard.html`. Requirements 43 and up were added in the design review on 2026-10-02.

## Requirements

### Projects

1. The system must provide a settings page to create and edit projects, each with a name, a local folder path, and a free-text agent prompt. The repository is not typed in: the app reads it with `git -C <local_path> remote get-url origin` and shows it read-only. Saving a project whose local folder is not a git repository must fail with an error.
2. A project must map to exactly one repository.
3. Agents for a project must launch with the project's local folder as their working directory and with all setting sources loaded (user, project, local), so the user's global `CLAUDE.md`, skills, plugins, hooks and MCP servers apply, as well as the repo's `CLAUDE.md`, `CLAUDE.local.md` and `.claude/` configuration.
4. The project's agent prompt must be delivered to every agent launched for that project, including resumes, appended to the Claude Code system prompt after the app contract (requirement 31). Each launch must send the current contract and project prompt, and the resumed agent must see the current versions, not the ones from its first turn (see Architecture Notes, prompt layering). Typical content: instructions to use the user's worktree skill, which also symlinks untracked local files (e.g. `CLAUDE.local.md`, `.env`) into new worktrees.
43. Each project gets a slug when it is created. The slug never changes, even when the project is renamed, because the project's artifact folder is named after it (requirement 22).
44. A project's local folder path must become read-only once any TODO in that project has been started, because Claude Code stores session transcripts under the folder the session was launched from, and a changed path would make every started TODO impossible to resume.
45. Projects cannot be deleted in v1.

### TODOs and sessions

5. The system must show a single page listing all TODOs across all projects. Each row shows the title, project, status, and branch or worktree name when one is in use. The branch is read with `git --no-optional-locks status --porcelain=v2 --branch` in the worktree path registered via `set_worktree` (requirement 46). A row with a worktree also shows a git line (commits on the branch, changed and untracked files, push state). A row with a worktree, or with a PR registered via `set_pr`, shows a PR line (number, state, conflicts, checks, reviews, open threads, behind base) that links to GitHub, as in design v3. Git state is read on every list fetch; PR state comes from one `gh api graphql` call per project every 2 minutes and after each run, run as whichever logged-in `gh` account can read the repository, without switching the active account. A `gh` failure keeps the last PR result, greyed, and shows the reason with a retry button.
6. Adding a TODO must require choosing a project, defaulting to the most recently chosen project, and a first instruction. A new TODO is created in status `not started yet` and no agent is launched.
7. A TODO's initial title must be its first instruction. Once the agent runs, it must replace the title with a generated one via `set_title`.
8. The user must start each TODO explicitly. Starting launches one Claude Code session with the first instruction and any messages committed before the start.
9. Each started TODO must correspond to exactly one Claude Code session, and its session id must be stored on the TODO. The app generates the session id and passes it on the first turn, so the id is known before the agent runs.
10. A TODO's status must be one of: `not started yet`, `running`, `waiting on you`, `idle`. `running` and `idle` are derived from the run lifecycle (requirement 30). `waiting on you` is set when the attention flag is raised and lasts until the flag is cleared. Clearing is the agent's responsibility, but the user must also be able to clear it manually. Each raise and clear is recorded with who did it (`user`, `agent` or `app`). A TODO has no "ended" state: `idle` is its resting state, and removing it is how it ends.
11. The system must not offer a pause action for a started TODO. `not started yet` exists only before the first start.
12. Removing a TODO must be possible in any status. If its agent is running, removal interrupts it, equivalent to pressing Esc in Claude Code. The TODO is marked with `removed_at` and `removed_by`, and it disappears from the main list together with its messages and files. Messages still queued at removal are kept.
13. The system must provide a view of removed TODOs.
14. Restoring a removed TODO must clear its removal, bring back its messages and files, and resume its session by session id. The continue message is built from any messages still queued at removal plus the user's uncommitted draft, sent as one prompt. Only when both are empty is it a plain "continue". A TODO removed before it was started is restored to `not started yet`, with its pending messages, and nothing launches.
15. If the session or the worktree of a TODO cannot be found during a restore, the system must show a warning and take no further action. It does not recreate the worktree or start a new session. The session check is `getSessionInfo(session_id)` returning nothing. The worktree check is the registered worktree path not existing on disk. The warning must mention that Claude Code deletes session transcripts after 30 days by default (`cleanupPeriodDays` in the user's `~/.claude/settings.json`).
16. A PR review must be handled as an ordinary TODO, e.g. "review PR #482" or a ticket reference. The existing review feature in auto-coder must be replaced by this flow rather than kept alongside it.
46. When an agent creates a worktree, it must register the worktree's absolute path via the `set_worktree` MCP tool. The app stores the path on the TODO. Every resume prompt must start with the line `Your worktree: <path>. Work there.` while a worktree is registered, because each run starts in the project's local folder, not in the worktree.
47. Every launch must add the project's worktree root to the session's additional directories, so the agent can work in the worktree and its `cd` persists across commands. The worktree root is `git -C <local_path> config worktree.root` if set, otherwise `<local_path>/../worktrees`.

### Chat

17. Each TODO must have a chat containing the user's messages and the agent's messages.
18. Agent chat messages must be one or two lines at most. Longer content must be written to a markdown file, and the message links to that file.
19. The message composer must follow a type, then commit flow. Nothing reaches the agent until the user commits. An uncommitted draft persists when the user navigates away and is shown with a draft marker. Drafts are stored on the server, because restore uses them (requirement 14).
20. A committed message must be delivered according to the session state:
    - When the agent is `running`, the message is queued and delivered as soon as the session goes idle, matching Claude Code's default behavior.
    - When the agent is `idle` or `waiting on you`, the message is delivered immediately by resuming the session.
    - When the TODO is `not started yet`, the message is stored and delivered at start.
    - Several messages waiting for the same delivery are sent as one prompt.
21. All messages must be stored permanently with author (`user`, `agent`, `user via operator`, `operator`, or `app`) and timestamp. `operator` is used for the operator's replies in its own chat. `app` is used for messages the app itself writes into a TODO's chat (requirements 48, 49, 50).
48. If a turn ends and the agent posted no message in it, the app must show the turn's final result text as the agent's message, cut to two lines. The full text is kept in the TODO's events.
49. If a run fails (the Claude Code process exits with an error, for example a usage limit), the app must post an `app` message naming the error, e.g. `claude exited 1: usage limit`, and set the TODO to `idle`. A run the app interrupted itself (removal, shutdown) produces no error message.
50. When a tool call is denied by the permission system, the app must post an `app` message such as `Auto mode blocked Bash: git push --force origin main` and raise the TODO's attention flag with actor `app`. The agent may clear the flag after the user's reply, as with any other attention.
51. A `running` TODO must show its most recent tool in the list and the TODO view, e.g. `PreToolUse · Bash`.
62. The user must be able to attach images and other files to a TODO's first instruction and to later messages, by pasting, dropping, or picking them. A follow-up may be attachments only. Each file is saved to `<artifact folder>/uploads/<message id>-<n>-<name>`, and the prompt lists the absolute paths after the message text, so the agent opens them with `Read`. The chat shows images as thumbnails and other files as download links with their size. A pasted image with the browser's generic name (`image.png`) is renamed `pasted-<HHMM>.png`. When the clipboard holds text and an image (a table copied from a document), the text pastes and the image is attached. Files are sent as base64 inside the JSON body, because the server refuses non-JSON POSTs. Attachments not yet committed are kept only in the browser tab. The operator chat takes no attachments.

### Markdown files

22. Agents must write plans, reviews, questions with context, and designs as markdown files in the TODO's artifact folder, `~/.switchboard/data/<project-slug>/<todo-id>/`. The folder's absolute path is provided in the app contract, so files survive worktree cleanup and stay out of git.
23. Agents must register each file they create against their TODO via `attach_file`. The TODO view lists its files. `attach_file` must reject a path outside the TODO's artifact folder.
24. The user must be able to open a file in the web app, edit it inline, save, and close it. Saving must not notify the agent. The user hands work back with a chat message such as "updated the review, post it but drop point 3".
25. A file must be read-only in the UI while its TODO's agent is `running`, and editable in any other status. If the TODO starts running while the user has unsaved edits (for example an operator message or a queued delivery resumes it), the edits must be kept in the browser and the editor locked. Saving is allowed again once the TODO stops running. If the agent changed the file on disk in the meantime, saving must warn that the file changed and offer Overwrite or Discard.
26. Files must be stored on disk. The database stores only the mapping between files and TODOs.
52. The file editor must show a plain text editor and a rendered markdown preview side by side. The read view shows the rendered markdown only.

### Agent launch and app contract

27. Agents must run in auto permission mode (`permissionMode: 'auto'`).
28. Hooks and the app's MCP server must be passed per run as options of the Agent SDK `query()` call (in-process hook callbacks and an in-process MCP server), not via the repository's settings, so only app-launched agents report to the app and the user's normal Claude Code sessions in the same repo are unaffected.
29. Worker agents must have these MCP tools: `post_message`, `set_title`, `set_attention` (raise and clear), `attach_file`, `set_worktree`, `set_pr` (the pull request a review works on; otherwise the PR is found by the worktree's branch). The contract must name them by their full tool ids (`mcp__app__post_message` etc.), because with the user's many MCP servers the agent loads tool definitions on demand.
30. Status must follow the run lifecycle:

    | Event | Status |
    |---|---|
    | A `query()` run starts (start, resume, queued delivery) | `running` |
    | The run ends, normally, with an error, or by interrupt | `idle`, or `waiting on you` if the attention flag is raised |

    The `UserPromptSubmit` and `PreToolUse` hooks are used only to record events and the most recent tool (requirement 51). The `Stop` hook must not be used to set `idle`, because it does not fire when a run is interrupted. `SessionEnd` must not change status.

31. Every agent must receive a fixed app contract that the user cannot edit, appended to the system prompt before the project prompt. The contract instructs the agent to:
    - use the app's MCP tools to communicate, and set a title on its first turn;
    - keep chat messages to one or two lines and put anything longer in a file under the given artifact folder, linked from the message;
    - treat the TODO text and the user's committed messages as its go-ahead for local edits within this TODO's scope, while outward actions (push, PR comments, Jira, Slack) still need an explicit message from the user;
    - work out from the TODO text, and from any referenced ticket, what kind of work this is (a review, a feature, or thinking only with no code changes) and whether it needs a worktree, following the project prompt for how to create one;
    - after creating a worktree, call `set_worktree` with its absolute path and work only there, ignoring any skill instruction not to switch context;
    - ask, via a message plus the attention flag, rather than guess when anything is unclear;
    - post a message and raise attention when auto mode blocks an action or it is unsure how to proceed, instead of routing around the block;
    - clear its own attention flag once the user's reply resolves it.
32. The system must not have a dispatcher or orchestrator pass. A started agent interprets its own TODO.
53. The app has no model setting. Agents use the model from the user's Claude Code settings.

### Operator

33. The page must show a pinned operator session above the TODO list, using the same chat UI.
34. The operator must have these tools: `list_sessions` (titles, projects, statuses, latest messages, optionally removed TODOs), `read_session` (a TODO's messages and file list), `read_file` (a TODO's markdown file by name), `create_todo`, and `send_message`, plus Claude Code's Read, Grep, Glob, WebSearch and WebFetch. It must not have Edit, Write, Bash, other MCP servers, or a tool to start TODOs. It runs with `~/.switchboard` as its working directory.
35. The operator must act only on the user's explicit instruction. It never polls, never answers a worker's question on its own, and never starts TODOs.
36. TODOs created by the operator must start as `not started yet`. The user starts them.
37. A message the operator sends on the user's behalf must appear in the target TODO's chat as a user message with a "via operator" marker.
38. The operator must get session state through its tools rather than accumulated conversation, so the user can start a fresh operator session at any time without losing anything.
39. Brainstorming, such as thinking through a future feature, must be possible in the operator or in a TODO the agent treats as thinking only.
54. Operator replies are shown in full, rendered as markdown. The one-or-two-line rule (requirement 18) does not apply to the operator.

### Attention

40. The main list must sort `waiting on you` TODOs to the top. Below them, TODOs are grouped in lanes in this order: `running`, `not started yet`, `idle`. Within each lane, the most recently updated TODO comes first.
41. The browser tab title must show the number of TODOs in `waiting on you`, e.g. `(2) switchboard`. Only `waiting on you` counts.
55. A TODO must show an Unread badge when an agent message arrived after the user last opened that TODO. Unread TODOs are not counted in the tab title and do not move to the waiting lane.

### Persistence

42. A local SQLite database must store projects, TODOs and their sessions, messages, drafts, attention changes, file mappings, and removals (`removed_by`, `removed_at`). This history must stay available for the operator to look up after a TODO is removed.
56. On shutdown, the app must interrupt all running agents. On startup, any TODO still marked `running` must be set to `idle` with an `app restarted` event. Messages still queued stay queued and are shown with a Deliver button. Nothing is delivered automatically on startup.
57. On first start, the app must import the projects (name, local path) from `~/.auto-coder/db.sqlite`, drop the old reviews and findings, and write the old stored review prompt to `~/.switchboard/old-review-prompt.md`, so the user can paste it into project prompts.

### UI

58. The UI follows `designs/Switchboard-v2.html`: the overview with the New TODO card, the pinned operator card and the status lanes; the Focus view of a TODO with Chat, Files and Events tabs and its "1 of 2 waiting" counter; the Removed view; the Projects page, which shows the fixed contract text read-only; and the Auto/Dark/Light theme buttons (Auto follows the OS setting).
59. Keyboard shortcuts: `N` focuses the new TODO input, `⌘J` opens the next TODO waiting on you, `⌘E` toggles file edit mode, `⌘S` saves the open file, `⌘↵` commits the draft, `Esc` closes the open file or goes back to the overview.
60. The design's "Show flow" panel (scenarios, lifecycle trace, live event feed) and the launch command preview on the Projects page are not built.
61. The UI gets live updates from the backend over SSE.

## Acceptance Criteria

- [ ] A project created in settings with name, local folder and agent prompt appears as a choice when adding a TODO, and shows the repository read from `git remote`.
- [ ] Saving a project whose local folder is not a git repository fails with an error.
- [ ] A project's local folder field is read-only once one of its TODOs has been started.
- [ ] The project picker for a new TODO defaults to the project chosen last time.
- [ ] A newly added TODO shows status `not started yet`, its first instruction as title, and no Claude process exists for it.
- [ ] Starting a TODO calls `query()` with the project's local folder as `cwd`, `permissionMode: 'auto'`, a pre-generated `sessionId`, the in-process hooks and app MCP server, the worktree root in `additionalDirectories`, the contract followed by the project prompt appended to the `claude_code` system prompt preset, and `extraArgs: {'system-prompt-snapshot': 'off'}`.
- [ ] After editing a project's prompt, the next resume of a started TODO in that project follows the new prompt.
- [ ] A Claude Code session started manually in the same repo produces no hook events or MCP traffic in the app.
- [ ] After the agent's first turn, the TODO's title is the generated title.
- [ ] While the agent's run is active, the TODO shows `running` and its most recent tool. After the run ends, it shows `idle`. After the agent raises attention, it shows `waiting on you` and sorts to the top of the list.
- [ ] The tab title count equals the number of TODOs in `waiting on you` and updates without a page reload.
- [ ] An agent message arriving while the user is not viewing that TODO shows an Unread badge, which clears when the user opens the TODO. The tab title count does not change.
- [ ] The user can clear a TODO's attention flag manually. The event history shows whether each raise and clear came from the user, the agent or the app.
- [ ] An agent that creates a worktree calls `set_worktree`. The TODO row then shows the worktree's branch, and the next resume prompt starts with `Your worktree: <path>. Work there.`
- [ ] A tool call denied by the permission system produces an `app` message naming the tool and command, and the TODO goes to `waiting on you`.
- [ ] A turn that ends without `post_message` shows the final result text, cut to two lines, as the agent's message.
- [ ] A screenshot pasted into the New TODO text or the chat composer is delivered with the message, shows as a thumbnail in the chat, and the agent can open it with `Read`.
- [ ] A run that fails produces an `app` message with the error and leaves the TODO `idle`.
- [ ] Removing a running TODO interrupts the agent, posts no error message, sets `removed_at` and `removed_by`, and hides the TODO, its messages and its files from the main list. It then appears in the removed view.
- [ ] Restoring that TODO shows it in the main list again with its messages and files, and resumes the same session id with "continue", or with the queued messages and draft as one prompt if any existed.
- [ ] Restoring a TODO whose worktree or session no longer exists shows a warning that mentions the 30-day transcript cleanup, and launches nothing.
- [ ] No UI control pauses a running TODO.
- [ ] A message committed while the agent is running is delivered exactly once, after the session goes idle. A message committed while idle is delivered immediately. Several queued messages are delivered as one prompt.
- [ ] Messages committed before start are delivered when the TODO starts.
- [ ] An uncommitted draft survives navigating away and back, and reloading the page, and is marked as a draft.
- [ ] Stopping the app while a TODO runs, then starting it again, shows that TODO as `idle` with an `app restarted` event. A message queued before the stop is still queued and shows a Deliver button.
- [ ] A file attached by an agent appears in the TODO's file list, is stored on disk in `~/.switchboard/data/<project-slug>/<todo-id>/`, and can be edited and saved inline. `attach_file` with a path outside that folder is rejected.
- [ ] A file's editor is read-only while its TODO is `running` and becomes editable once the TODO leaves `running`.
- [ ] Unsaved file edits survive the TODO starting to run. Saving after the agent changed the file warns and offers Overwrite or Discard.
- [ ] Saving a file sends nothing to the agent.
- [ ] A PR review request entered as a TODO produces its findings as a markdown file and only a short message in the chat.
- [ ] Asking the operator to create a TODO produces a TODO in `not started yet` under the right project, and nothing starts.
- [ ] Asking the operator to send an instruction to a TODO puts a user message marked "via operator" in that TODO's chat.
- [ ] Starting a fresh operator session and asking about existing TODOs returns the same information as before.
- [ ] When a worker raises a question, the operator does not respond to it unless the user asks it to.
- [ ] The operator can report on a removed TODO's history.
- [ ] On first start, projects from `~/.auto-coder/db.sqlite` appear in the Projects page and the old review prompt is in `~/.switchboard/old-review-prompt.md`.
- [ ] Each UI flow above has been checked in Chrome against the running app on localhost.

## Architecture Notes

- **Shape.** One local Bun process, bound to 127.0.0.1 (the existing Host header check in `server.ts` stays), owns the agent runs, the SQLite database (`bun:sqlite`) and an SSE stream for the browser UI. The frontend stays a single `index.html` with no build step, using the `marked` and `dompurify` already loaded from cdnjs.
- **Run model (verified in the spike).** One Agent SDK `query()` call per turn, with `resume: <session_id>` after the first. This is what today's `runClaude` in `server.ts` does. The run ending (the `for await` loop finishing or throwing) is the idle signal, so queued messages are sent when it ends, and interrupting a run means calling `interrupt()` on its query. Hooks are in-process callbacks passed in `options.hooks`. The app's MCP server is created with `createSdkMcpServer` and passed in `options.mcpServers`. No HTTP hook endpoint, no MCP HTTP server, no temporary settings files. Each callback and tool handler already knows its TODO, so nothing has to be matched by `session_id`. The app's own MCP tools are listed in `allowedTools`, so auto mode never asks about them (in the spike they were listed; running without that list was not tested).
- **Prompt layering (verified in the spike).** The contract and project prompt go in `systemPrompt: { type: 'preset', preset: 'claude_code', append: <contract + project prompt> }`. The SDK has no `appendSystemPrompt` option; passing one is silently ignored. By default Claude Code records the system prompt on a session's first request and reuses it on every resume, so a changed project prompt would never reach a resumed agent. Passing `extraArgs: { 'system-prompt-snapshot': 'off' }` renders it fresh on every launch. The contract is versioned with the app. The project prompt is user content.
- **Denials (verified in the spike).** A denied tool call arrives as an SDK message `{type: 'system', subtype: 'permission_denied', tool_name, message}` and in `result.permission_denials`, and the agent receives the denial as an error tool result. The `PermissionDenied` hook did not fire, so requirement 50 uses the SDK message.
- **Interrupt (verified in the spike).** `interrupt()` during a Bash call kills the command, gives the agent the tool result "The user doesn't want to proceed with this tool use…", and makes the `for await` loop throw with result subtype `error_during_execution`. `Stop` does not fire. The transcript is kept, and resuming the same session id works: the agent knows its last command was stopped.
- **Attention is deterministic.** "What needs me" comes from run status, attention flags and denials, not from an LLM. The operator plays no part in signaling. This is deliberate: a Paperclip-style CEO agent watching all sessions would cost usage continuously, could paraphrase or miss things, and would rebuild the long-thread problem this design avoids. Here the user is the CEO, and the operator is a chief of staff that only acts when asked.
- **Data sketch.** `projects(id, name, slug, local_path, agent_prompt)`; `todos(id, project_id, title, status, attention, session_id, worktree_path, last_read_at, created_at, updated_at, removed_at, removed_by)`; `messages(id, todo_id, author, body, state, committed_at, delivered_at, file_id)` where `state` is `pending`, `queued` or `delivered`; `drafts(todo_id, body)`; `files(id, todo_id, path)`; `events(todo_id, kind, actor, detail, at)` for run, hook, attention, denial and removal history, where `actor` is `user`, `agent`, `app` or `operator`. The operator uses `todo_id = NULL` for its own messages; a fresh operator session starts a new session id and hides older operator messages.
- **Data folder.** `~/.switchboard/` holds the database, `data/<project-slug>/<todo-id>/` for artifact files, and `old-review-prompt.md` from the migration.
- **Rename.** The folder, `package.json` name, UI title and GitHub repository are renamed to switchboard (earlier: agent-coordinator). The GitHub rename is done by the user. The folder rename happens last, after the code work.
- **Build order.** Spike (done, see below), then this spec, then the build on branch `feature/agent-coordinator` with local commits only, verified in Chrome against the app on localhost.

## Spike Findings (2026-10-02)

Throwaway script run against a test repository in the session scratchpad, on `claude-sonnet-5-5` with the user's full Claude Code config loaded. Total cost at least $5.12 at API prices for about 15 turns, or $0.2–0.8 per turn.

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

## Open Questions

- Does the auto-mode classifier itself deny anything under SDK `query()`? The one denial seen had reason `subcommandResults` (a compound command), not `classifier`. A single `git push --force origin main` and `curl … | sh` were both allowed, and an exfiltration-style command was refused by the model before any tool call. The design does not depend on the answer, because requirement 50 reacts to any denial.
- In one spike run, the agent passed the parent `worktrees/` folder to the worktree skill's `copy-local-settings.sh`, so `.env` was linked next to the worktree instead of inside it, and the agent then reported success. The user's real worktrees have their `.env` links in the right place. Watch for this in practice; the fix would be one word in the skill's step 5.

## Out of Scope (v1)

- Native or macOS notifications. The tab title count is the only signal for now.
- Cleaning up worktrees, branches or sessions. The user does this manually, or as a TODO using the git-tidy skill.
- Recreating a missing worktree or session on restore.
- Concurrency caps, queueing of starts, and usage-limit handling. Agents that hit the limit go idle, and the user checks usage in Claude.
- Guarding against two TODOs sharing a worktree or branch. Overlap is sometimes intentional; showing the branch on each row is the only aid.
- Pausing a running TODO.
- Routing permission prompts to the UI.
- Operator autonomy: answering workers, starting TODOs, or acting without an instruction.
- Projects spanning multiple repositories.
- A dispatcher or orchestrator pass.
- Restricting worker agents beyond the user's normal Claude Code config.
- Deleting projects.
- A model setting.
- Changing `cleanupPeriodDays`. Raising it is the user's choice in their own settings.
- The design's "Show flow" panel and launch command preview.
