# Design: repos instead of projects, and where each session runs

Status: decided, 2026-10-07 (behaviour questions resolved, see section 7; two visual questions stay with Claude Design). Input for Claude Design (a v5 of `designs/Switchboard-v4.html`) and for new and rewritten items in `designs/feature-spec-switchboard.md`.

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
| **Coordinator** | Whoever decides which repo the next session runs in: the live agent proposes, the user confirms. See section 3. |
| **Proposal** | A ready-made next session, left by the live agent: the repo to run in and the first message. The user starts it with one click, or edits or dismisses it. |

Fixed rule, from Claude Code itself: **a session's working directory cannot change after it starts.** Claude Code stores the transcript under the folder a session was launched from (items 44, 78), and resume only works from there. So a session's repo is chosen once, when the session starts. Moving to another repo always means a new session. A worktree inside the same repo can still be registered mid-session, as today (item 46).

## 3. Decision: the live agent proposes, the user confirms

The TODO text says "the coordinator makes sure to do the work in the correct worktrees / repos". The spec has no coordinator today: the user starts every session (item 71), and "a dispatcher or orchestrator pass" is listed under *Not built on purpose*. Three readings were on the table: the app alone with a repo pick for the user (A), the live agent proposing and the user confirming (B), and a coordinator agent that fires sessions on its own (C, the orchestrator the spec rejects).

**Decided: B.** The agent does the routing work, the app fires the session on the user's one-click confirm, so control stays with the user and "a dispatcher or orchestrator pass" stays under *Not built on purpose*. How it works:

- A new MCP tool **`propose_session(repo, message)`** lets the live agent leave a proposal: the repo the next session should run in (by name; the TODO's own repos or any other repo in Settings) and the first message for it, e.g. "Server part is done on branch ai-1234-mcp, PR #512. Build the client against it: …". A second call replaces the first; `repo` must exist, else the tool returns an error naming the known repos.
- A TODO holds **at most one proposal**, and it belongs to the session that made it. It survives the session's end (that is its point: "the server part is done, the client part belongs in `sb-web`") and is cleared when a new session starts, whether or not it took the proposal.
- The **New session composer** (item 71) shows the proposal as a prefilled form: "Runs in" set to the proposed repo, the message in the field, a line "Proposed by the agent in session n". The user starts it as is, edits the repo or the message first, or dismisses it and starts from blank.
- The **board card** shows a proposal on a TODO whose session has ended as a small line ("next: sb-web"), so a waiting hand-over is visible without opening the TODO. The Focus header info line shows it too.
- The agent may also propose the **same repo** (a plain "next step" hand-over). The tool is about the next session, not only about moving repos.
- The per-turn instructions tell the agent when to use it: at the end of a step whose follow-up belongs in another repo, or when the user asked for a hand-over. Not for every end of a turn.
- The **operator** reads proposals in `read_todo` and may make one through `create_todo`'s first repo (a not-started TODO with a first repo and a draft is the same shape).

## 4. Model changes (behaviour level)

1. **Repos** replace projects, 1:1 at migration. Settings › Projects becomes Settings › Repos. Name, folder, origin, agent prompt and worktree root as today.
2. **A TODO has no repo of its own.** Before it starts it has a **first repo**: the repo session 1 will run in. It may be empty for thinking-only TODOs, in which case session 1 runs in the artifact folder and its terminals (item 82) open there too. A later session of such a TODO may pick a repo; the TODO then has a location like any other.
3. **Each session has a repo.** Session 1 takes the first repo; each New session picks one (default: the live session's repo).
4. **Worktrees and PRs are per repo, per TODO.** `set_worktree` registers a worktree for the session's repo; a TODO has at most one worktree per repo, and a second `set_worktree` in the same repo replaces the first (as today). A review checkout next to a feature branch in the same repo is a second TODO. A later session in the same repo is handed that worktree; a session in another repo is told about it (path and branch) but works in its own. `set_pr` likewise records the PR against the session's repo. A TODO can therefore have e.g. `sb-api · ai-1234-mcp · PR #512` and `sb-web · ai-1234-mcp-client · PR #88`.
5. **The TODO's locations** are the set of (repo, worktree) pairs its sessions have used. Cards, filters, files and terminals are built from them.
6. **Agent prompt**: a session gets its own repo's prompt only (item 4, per session instead of per TODO).
7. **Additional directories** (item 47): the session's repo's worktree root, the artifact folder, and the folders of every other location the TODO has, so a session in `sb-web` can read the branch it builds on in `sb-api`. That also lets it edit there. The contract says other repos are for reading: changes there belong in a session of that repo, which is what `propose_session` is for. Nothing enforces it beyond the instructions, as with the worktree line today.
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
12. **Operator**: `list_todos` and `read_todo` report locations, each session's repo and a waiting proposal; `create_todo` takes an optional first repo.
13. **Proposals** (section 3): `propose_session(repo, message)` for the TODO's agent; one per TODO; shown in the composer, on the card and in the info line; cleared when the next session starts.
14. **Repo filter** (items 63, 90): a TODO matches a repo chip if any of its locations, or its first repo, is in that repo. A two-repo TODO shows under both chips, so chip counts can add up to more than the total.

Spec items to rewrite: 1, 2, 3, 4, 6, 15, 22, 43, 44, 45, 46, 47, 57, 63, 71, 73, 78, 82, 88, 90, 94, 96, 101, 110, 111, 114, 124. New items: proposals (tool, composer, card line). To strike from *Not built on purpose*: "Projects spanning multiple repositories". "A dispatcher or orchestrator pass" stays.

## 5. Brief for Claude Design: where is each session running?

The one new visual idea the design needs is a **location mark**: a compact, recognisable token for "repo + folder", used identically everywhere. Suggested anatomy: repo name, then branch (worktree) or `main checkout`, e.g. `sb-api ⎇ ai-1234-mcp` or `sb-web · main checkout`. Considerations for the designer:

- Repos need to be told apart at a glance across many cards. Options: a stable per-repo colour or glyph (assigned in Settings › Repos), or just the name in a consistent spot. The palette is already busy (status cyan/amber/violet, git and PR colours), so a per-repo colour must not collide with status meaning. **Design question D1**, for the designer: leaning name only, in a fixed spot and a fixed type style, since the common case is one repo and the name is what the user knows; a colour field is worth it only if the mock with 2+ repos reads badly without it.
- "Live" vs "earlier": the live session's location is the one that matters now. Other locations of the TODO are context and should read as secondary.
- `main checkout` is a weaker, shared place (other TODOs may work there too, item 124's "project folder, shared with other TODOs"). It deserves a subtle warning tone, not an alarm.
- A TODO with one repo (the common case) must look no busier than today.

### Surfaces, and what each must show

| Surface | Today | New |
|---|---|---|
| **Board card** (item 5) | project name, branch, git line, PR line, `· session n` | Live location mark replaces the project name. With 2+ repos, the other repos as small secondary marks (or `+1 repo`). Git and PR lines are the live location's. **Design question D2**: with 2+ PRs, one PR line per repo, or the live one plus a summary (`+1 PR`) that takes the worst state of the others (a failing check in the non-live repo turns the summary red)? Leaning the summary, so a one-repo card stays as today and a two-repo card grows by one short line. A waiting proposal adds a `next: <repo>` line. |
| **Backlog row** (item 88) | project name | First repo (or "no repo"). |
| **New TODO card** (item 6) | project select | First repo select, optional. Default from the filter, else last used. |
| **Repo filter row** (items 63, 90) | one chip per project | One chip per repo. A TODO matches a repo if any of its locations (or its first repo) is in it, so counts can add up to more than the total. Design how that reads. |
| **Focus header / info line** (item 124) | project · session · branch · git · PR | Live session's location mark first, then session, git, PR as today; a waiting proposal as `next: <repo>` after the session has ended. A TODO with 2+ repos gets a way to see all its locations (popover or a row of marks), each with branch, git summary and PR. Before start: first repo is an editable select (replaces the project select, item 94). |
| **Chat session timeline** (item 76) | `SESSION n` header with label, time, id | Each session header carries its location mark. When the repo changes between sessions, the boundary should be visibly stronger ("moved to sb-web"), since that's exactly the "where is it running now" moment. A one-session TODO still has no header, so the location must be clear from the Focus header alone. |
| **New session composer** (item 71) | heading, placeholder, "Handed over: …" | Adds a **"Runs in"** pick: the TODO's repos first (with their worktree, "reuse worktree ai-1234-mcp"), then all other repos ("new in this repo; the agent creates a worktree if needed"). The hand-over hint names what carries over to the chosen repo. A proposal (section 3) prefills the pick and the message and shows "Proposed by the agent in session n", with a way to dismiss it. |
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
5. New session composer with the repo pick open, and the same composer prefilled by an agent proposal.
6. Backlog TODO with no first repo (thinking only).
7. A location whose folder is gone (worktree removed after merge): how it greys out on the card, in Files and in the Terminal menu.

## 6. Data and migration (implementation sketch)

- Rename table `projects` → `repos` (or keep the table and rename in UI only; renaming is cleaner for the operator's tools and the code).
- `todos.project_id` → `todos.first_repo_id` (nullable).
- `sessions` gains `repo_id`.
- New `locations (id, todo_id, repo_id, worktree_path, pr_number, created_session_no)`, unique on `(todo_id, repo_id)`. Replaces `todos.worktree_path` and `todos.pr_number`.
- `todos` gains the proposal: `proposal_repo_id`, `proposal_message`, `proposal_session_no` (all nullable, cleared together when a session starts).
- Migration: every existing session gets its TODO's `project_id`; every TODO with a worktree or PR gets one `locations` row; artifact folders move to `data/<todo-id>/`. All additive except the folder move, which needs a one-time, logged step.

## 7. Decisions

Resolved 2026-10-07, all on the proposed line:

1. **Coordinator**: the live agent proposes, the user confirms (section 3). No orchestrator.
2. **One worktree per repo per TODO.** A second worktree in the same repo is a second TODO (section 4, item 4).
3. **No-repo TODOs** are allowed: session 1 runs in the artifact folder, terminals open there (section 4, item 2).
4. **Filter semantics**: a repo chip matches a TODO on any of its locations or its first repo (section 4, item 14).
5. **Other repos are read only by contract**; changes in another repo come from a session that runs there (section 4, item 7).

Left with Claude Design, with a leaning each, see section 5: **D1** per-repo colour or glyph versus name only, **D2** every PR line versus live PR plus a summary.
