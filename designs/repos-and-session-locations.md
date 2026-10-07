# Design: repos instead of projects, and where each session runs

Status: draft for discussion, 2026-10-07. Input for Claude Design (a v5 of `designs/Switchboard-v4.html`) and, once agreed, for new and rewritten items in `designs/feature-spec-switchboard.md`.

## 1. Why

Today a TODO belongs to one **project**, a project is exactly one repository (spec item 2), and every session of a TODO launches in that project's folder (item 3). Real tickets cross repos: an API change plus its client, a library fix plus the app that pins it, a review in one repo and a follow-up in another. Today that takes two TODOs, two cards and two chats for one ticket.

The change: drop "project" as the TODO's owner. Switchboard knows a set of **repos**. A TODO is the ticket; each of its **sessions** runs in one repo, and the next session can run in a different one. The work stays one card, one chat timeline, one set of files.

The cost is that "where is this running?" is no longer a property of the TODO. It is a property of each session, and it can change mid-ticket. The UI has to make that obvious everywhere the user looks, which is what this document is mostly about.

## 2. Concepts

| Term | Meaning |
|---|---|
| **Repo** | A git repository on disk that Switchboard may launch sessions in. Name, local folder (main checkout), origin URL (read, not typed), agent prompt, worktree root, slug. Replaces Project. |
| **TODO** | The ticket. Owns title, chat, files, status, attention, draft, and its **locations**. No longer owned by a repo. |
| **Session** | One Claude Code session of the TODO, as today. New: it has exactly one **repo**, fixed when it starts. |
| **Location** | Where work happens: a repo plus a folder in it, either the repo's main checkout or a worktree (with its branch). A TODO collects locations as its sessions register worktrees. |
| **Coordinator** | Whoever decides which repo the next session runs in. See section 3; this is the main open question. |

Fixed rule, from Claude Code itself: **a session's working directory cannot change after it starts.** Claude Code stores the transcript under the folder a session was launched from (items 44, 78), and resume only works from there. So a session's repo is chosen once, when the session starts. Moving to another repo always means a new session. A worktree inside the same repo can still be registered mid-session, as today (item 46).

## 3. Open question: who is the coordinator?

The TODO text says "the coordinator makes sure to do the work in the correct worktrees / repos". The spec has no coordinator today: the user starts every session (item 71), and "a dispatcher or orchestrator pass" is listed under *Not built on purpose*. Three readings:

- **A. The app, steered by the user.** New session gets a repo pick. The user chooses; the app launches there and hands over. Smallest change, keeps "streamline the user's work, don't delegate their control".
- **B. The live agent proposes, the user confirms.** A new MCP tool, e.g. `propose_session(repo, message)`, lets the agent say "the server part is done, the client part belongs in `sb-web`". The app shows that as a ready-made New session (repo and first message prefilled) that the user starts with one click. Still user-started, but the agent does the routing work.
- **C. A coordinator agent** per TODO (or global) that plans the steps and fires sessions in the right repos on its own. This is the orchestrator the spec rejects today, and it would start runs without the user.

The TODO text has the coordinator firing sessions, which A fits least. **Leaning B**: the agent does the routing, the app fires the session on the user's one-click confirm, so control stays with the user. The UI below works for A and B (B adds the proposal to the New session composer). C would need its own design document. *Waiting for your answer before this is final.*

## 4. Model changes (behaviour level)

1. **Repos** replace projects, 1:1 at migration. Settings › Projects becomes Settings › Repos. Name, folder, origin, agent prompt and worktree root as today.
2. **A TODO has no repo of its own.** Before it starts it has a **first repo**: the repo session 1 will run in. It may be empty for thinking-only TODOs, in which case session 1 runs in the artifact folder (see open questions).
3. **Each session has a repo.** Session 1 takes the first repo; each New session picks one (default: the live session's repo).
4. **Worktrees and PRs are per repo, per TODO.** `set_worktree` registers a worktree for the session's repo. A later session in the same repo is handed that worktree; a session in another repo is told about it (path and branch) but works in its own. `set_pr` likewise records the PR against the session's repo. A TODO can therefore have e.g. `sb-api · ai-1234-mcp · PR #512` and `sb-web · ai-1234-mcp-client · PR #88`.
5. **The TODO's locations** are the set of (repo, worktree) pairs its sessions have used. Cards, filters, files and terminals are built from them.
6. **Agent prompt**: a session gets its own repo's prompt only (item 4, per session instead of per TODO).
7. **Additional directories** (item 47): the session's repo's worktree root, the artifact folder, and the folders of every other location the TODO has, so a session in `sb-web` can read the branch it builds on in `sb-api`. That also lets it edit there. Proposed: the contract says other repos are for reading, and changes there belong in a session of that repo (open question 7).
7a. **Per-turn worktree line** (item 46): "Your worktree: … Work there." names the live session's worktree in its own repo, not a TODO-wide one.
7b. **Restore checks** (item 15) look at the live session's location: its repo folder and its worktree.
8. **Hand-over header** (item 73) gains the location, and the other repos' state:
   ```
   This TODO continues earlier work (session 3). Title: Implement MCPs (AI-1234). Keep this title.
   This session runs in repo sb-web (/home/me/code/sb-web).
   Your worktree in this repo: /home/me/code/worktrees/ai-1234-mcp-client. Work there.
   Other repos of this TODO:
     sb-api: worktree /home/me/code/worktrees/ai-1234-mcp, branch ai-1234-mcp, PR #512 (sessions 1–2)
   Last message of session 2: Server done, client stubs left.
   Files from earlier sessions: …
   ```
9. **Artifact folder** moves from `data/<project-slug>/<todo-id>/` to `data/<todo-id>/`, since a TODO no longer has one repo. Migration moves the folders and rewrites `files.path` and `attachments.path`. Item 96 shrinks: changing the first repo before the start moves no folder, but the 409 while a terminal is alive stays, because a not-started TODO's terminals open in the first repo's folder.
10. **Resume outside the app** (item 78): the copy tooltip names the session's own repo folder, which now differs per session.
11. **PR polling** (item 5) runs per repo, over the TODO's locations, not per project.
12. **Operator**: `list_todos` and `read_todo` report locations and each session's repo; `create_todo` takes an optional first repo.

Spec items to rewrite: 1, 2, 3, 4, 6, 15, 22, 43, 44, 45, 46, 47, 57, 63, 73, 78, 82, 88, 90, 94, 96, 101, 110, 111, 114, 124. To strike from *Not built on purpose*: "Projects spanning multiple repositories". "A dispatcher or orchestrator pass" stays unless C is chosen.

## 5. Brief for Claude Design: where is each session running?

The one new visual idea the design needs is a **location mark**: a compact, recognisable token for "repo + folder", used identically everywhere. Suggested anatomy: repo name, then branch (worktree) or `main checkout`, e.g. `sb-api ⎇ ai-1234-mcp` or `sb-web · main checkout`. Considerations for the designer:

- Repos need to be told apart at a glance across many cards. Options: a stable per-repo colour or glyph (assigned in Settings › Repos), or just the name in a consistent spot. The palette is already busy (status cyan/amber/violet, git and PR colours), so a per-repo colour must not collide with status meaning.
- "Live" vs "earlier": the live session's location is the one that matters now. Other locations of the TODO are context and should read as secondary.
- `main checkout` is a weaker, shared place (other TODOs may work there too, item 124's "project folder, shared with other TODOs"). It deserves a subtle warning tone, not an alarm.
- A TODO with one repo (the common case) must look no busier than today.

### Surfaces, and what each must show

| Surface | Today | New |
|---|---|---|
| **Board card** (item 5) | project name, branch, git line, PR line, `· session n` | Live location mark replaces the project name. With 2+ repos, the other repos as small secondary marks (or `+1 repo`). Git and PR lines are the live location's; with 2+ PRs, decide: one PR line per repo, or live one plus `+1 PR`. |
| **Backlog row** (item 88) | project name | First repo (or "no repo"). |
| **New TODO card** (item 6) | project select | First repo select, optional. Default from the filter, else last used. |
| **Repo filter row** (items 63, 90) | one chip per project | One chip per repo. A TODO matches a repo if any of its locations (or its first repo) is in it, so counts can add up to more than the total. Design how that reads. |
| **Focus header / info line** (item 124) | project · session · branch · git · PR | Live session's location mark first, then session, git, PR as today. A TODO with 2+ repos gets a way to see all its locations (popover or a row of marks), each with branch, git summary and PR. Before start: first repo is an editable select (replaces the project select, item 94). |
| **Chat session timeline** (item 76) | `SESSION n` header with label, time, id | Each session header carries its location mark. When the repo changes between sessions, the boundary should be visibly stronger ("moved to sb-web"), since that's exactly the "where is it running now" moment. A one-session TODO still has no header, so the location must be clear from the Focus header alone. |
| **New session composer** (item 71) | heading, placeholder, "Handed over: …" | Adds a **"Runs in"** pick: the TODO's repos first (with their worktree, "reuse worktree ai-1234-mcp"), then all other repos ("new in this repo; the agent creates a worktree if needed"). The hand-over hint names what carries over to the chosen repo. Under option B, the agent's proposal prefills this and shows "Proposed by the agent". |
| **Files root switcher** (item 101) | TODO files · Worktree · Project | TODO files, then per location: `<repo> ⎇ <branch>` (worktree) and `<repo> · main`. Live location first. Could get long with 3+ repos; a grouped dropdown may be needed. |
| **Terminal + menu** (item 111) | "Worktree · branch", "Project folder · path" | One group per location, live first, same location marks. Each terminal tab's footer shows its location mark. |
| **Claude terminal context file** (item 114) | project, worktree, branch, git and PR line | Every location with its branch, git and PR line, live first, and each session's repo. Not drawn, but its content follows the same model. |
| **Removed row** (item 13) | project name | Location marks, as on the card. |
| **Settings › Repos** (items 1, 122) | Projects list with repo and TODO count | Same fields renamed; plus the repo's colour/glyph if the design uses one; TODO count means "TODOs with a location here". |
| **Operator card / info line** | `operator · all projects` | `operator · all repos`. Tool lines show `create_todo(repo: "sb-api")`. |
| **Title bar** | — | No change. Counts are TODOs, not repos. |

### States the design should cover

1. One-repo TODO in its main checkout (today's common "no worktree" case).
2. One-repo TODO with a worktree and PR (today's common case). Must look like today.
3. Two-repo TODO, live session in the second repo, both with worktrees and PRs, one PR failing checks in the *non-live* repo. Where does that failure surface?
4. Session timeline with sessions 1–2 in `sb-api` and session 3 in `sb-web`.
5. New session composer with the repo pick open (and, for B, with an agent proposal).
6. Backlog TODO with no first repo (thinking only).
7. A location whose folder is gone (worktree removed after merge): how it greys out on the card, in Files and in the Terminal menu.

## 6. Data and migration (implementation sketch)

- Rename table `projects` → `repos` (or keep the table and rename in UI only; renaming is cleaner for the operator's tools and the code).
- `todos.project_id` → `todos.first_repo_id` (nullable).
- `sessions` gains `repo_id`.
- New `locations (id, todo_id, repo_id, worktree_path, pr_number, created_session_no)`, unique on `(todo_id, repo_id)`. Replaces `todos.worktree_path` and `todos.pr_number`.
- Migration: every existing session gets its TODO's `project_id`; every TODO with a worktree or PR gets one `locations` row; artifact folders move to `data/<todo-id>/`. All additive except the folder move, which needs a one-time, logged step.

## 7. Other open questions

1. **Coordinator** (section 3). Blocks the New session part of the design.
2. **One worktree per repo per TODO**, or may a TODO have two worktrees in one repo (e.g. a review checkout next to a feature branch)? Proposed: one; a second needs a second TODO.
3. **No-repo TODOs**: should a thinking-only TODO launch in the artifact folder, or must every session have a repo? Proposed: allow no repo, cwd = artifact folder; its terminals (item 82) open there too.
4. **Per-repo colour or glyph**: worth the Settings field, or is the name enough?
5. **Card density**: with 2+ PRs, show every PR line, or only the live one plus a summary?
6. **Filter semantics**: match on any location (proposed) or only on the live one?
7. **Other repos: read only, or editable?** A session can reach the TODO's other worktrees. Proposed: read only by contract, so each repo's changes come from a session that runs there and shows its location.
