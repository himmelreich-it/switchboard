# Design: the operator picks the repo at start (v1)

Status: decided, 2026-10-09. v1 is deliberately small, to test whether operator routing works and feels right. Input for Claude Design (a v5 of `designs/Switchboard-v4.html`) and for rewritten items in `designs/feature-spec-switchboard.md`. The multi-repo model of the earlier draft is parked under section 8.

## 1. Why

Today you pick a project when you create a TODO. That is a decision made before the ticket has been looked at, and it is a chore. What you want instead:

- **Create a TODO with no repo.** Write what needs doing, nothing else.
- **On Start, the operator works out which repo it belongs to** and the first session starts there. When it can't tell, it asks you to choose from a few sensible options.
- You stay in control: the choice is visible, you can stop it, and you can change it while nothing has been built yet.

v1 keeps **one repo per TODO**. A later session runs in the same repo; there is no switching between repos (section 8).

## 2. Concepts

| Term | Meaning |
|---|---|
| **Repo** | Replaces Project, 1:1. A git repository Switchboard may launch sessions in: name, local folder, origin (read, not typed), agent prompt, worktree root, slug, and now a **profile**. |
| **Profile** | A short text per repo that says what lives there (services, languages, main folders, what kind of tickets land there). An agent writes it when the repo is added. You can edit it and Rescan it. The operator routes on it. |
| **Routing** | The step between pressing Start and the first session: the operator picks the repo, picks "no repo", or asks you. |
| **TODO number** | The TODO's id, shown as `T-42`. Lets you and agents refer to another TODO. |

## 3. Routing at start

1. **New TODO takes no repo.** It takes the first instruction and attachments, as today, minus the project select. A not-started TODO has no repo.
2. **Start runs routing.** It is a short operator run for that TODO, separate from the pinned operator chat. It gets the TODO text and attachments' names, notes waiting for the start, the list of repos with their profiles, and read access to the repo folders (Read, Grep, Glob) and to other TODOs (`read_todo`, `read_file`). A mention like `T-38` points it at that TODO and its repo.
3. **It ends in exactly one of three ways**, each a tool call:
   - `route_todo(repo, reason)`: the app posts an operator line in the TODO chat, "Runs in **sb-api**: the MCP server lives there.", and starts session 1 in that repo straight away. No confirm click.
   - `route_todo(none, reason)`: thinking only. Session 1 runs in the TODO's artifact folder, as a no-repo TODO.
   - `ask_repo(question, options)`: the operator posts the question in the TODO chat with 2–3 options as buttons (plus "No repo"), and raises attention. A click, or a typed reply, ends routing and starts session 1. A typed reply that isn't a clear choice goes back to the operator for one more routing turn.
4. **While routing runs** the TODO shows as running, with the status text "Picking a repo". Stop cancels routing and leaves the TODO not started.
5. **The operator's power grows on purpose.** Spec item 35 ("never starts TODOs") changes to: the operator never starts a TODO on its own; when you press Start, it decides where the TODO runs and the app starts it there.
6. **Routing is visible in the chat** as operator messages (violet, like the operator elsewhere), with the tool line from item 127 (`→ route_todo(repo: "sb-api")`). The event log shows what it read.

## 4. Changing the repo afterwards

- **"Change repo"** is allowed while the TODO has **no worktree registered** (and, as today, no running terminal: 409). It sits in the Focus header where the project select was for not-started TODOs (item 94).
- Since a session's folder is fixed at launch, changing the repo ends the live session. The next message starts a new session in the new repo, with the normal hand-over header. Nothing moves on disk: the artifact folder no longer depends on the repo (section 6).
- The same control lets a no-repo TODO get a repo later, as long as it has no worktree. Once a worktree exists, the repo is fixed; a wrong repo at that point means a new TODO.

## 5. Profiles

- **Written by an agent when a repo is added**, and on **Rescan**. It's a read-only Claude run in the repo folder with a prompt aimed at routing: what this repo is, its main parts and where they live, its stack, and which kinds of tickets belong here and which don't. The output is a short markdown text (target under 300 words).
- **Editable** in Settings › Repos. Editing marks it as edited; Rescan asks before overwriting an edited profile.
- Settings shows `Profile · scanned 3d ago · Rescan` (or `· edited`), and a running scan shows "Scanning…".
- A repo without a profile can still be routed to, but on name and folder only. The operator says so when it asks.

## 6. TODO numbers, and reading other TODOs

- Every TODO shows **`T-<id>`**: on the board card, the backlog row, the Focus header (click to copy, toast "Copied T-42") and the Removed row. The operator uses the same form in its replies and tool lines.
- **Workers get read-only `read_todo(id)` and `read_file(id, name)`**, the same output as the operator's. They don't get other TODOs' folders as additional directories.
- The per-turn instructions tell agents (the worker and the routing operator) that `T-<n>` in text refers to a TODO and can be read with those tools. There's no link UI in v1.
- Jira keys (`AI-1234`) stay plain text in the TODO. `T-42` is Switchboard's own number, and the two are not linked.

## 7. Model and spec changes

1. **Projects become Repos**, 1:1 at migration, with the same fields plus a profile. Settings › Projects becomes Settings › Repos. Profiles of migrated repos are empty until the first Rescan; the app offers "Scan all" once.
2. **`todos.project_id` becomes `todos.repo_id`, nullable.** It is empty before the start and for no-repo TODOs.
3. **Artifact folder** moves from `data/<project-slug>/<todo-id>/` to `data/<todo-id>/` for all TODOs. This is a one-time, logged migration that also rewrites `files.path` and `attachments.path`. Terminals of a not-started or no-repo TODO open in the artifact folder. Item 96 (move the folder on project change) goes away.
4. **Launch** (items 3, 4, 47): as today, in the TODO's repo. A no-repo TODO launches in its artifact folder, with no repo prompt and no worktree root.
5. **Repo filter row** (items 63, 90): one chip per repo plus a **No repo** chip. Not-started TODOs have no repo yet, so the backlog hides the filter row, and the New TODO card no longer takes a preselected repo from it.
6. **Operator tools**: `list_todos` and `read_todo` show `T-<id>` and the repo. `create_todo` takes no project. New tools for routing only: `route_todo`, `ask_repo`.
7. **New TODO card** (item 6): no project select. Its note reads "Lands in the backlog. The operator picks the repo when you start it."

Spec items to rewrite: 1, 2, 3, 4, 6, 13, 15, 22, 34, 35, 36, 43, 44, 47, 57, 63, 82, 88, 90, 94, 96, 114, 122, 124, 127. New items: routing (section 3), Change repo (section 4), profiles (section 5), TODO numbers and worker `read_todo` (section 6). Remove from *Not built on purpose*: nothing. "A dispatcher or orchestrator pass" stays: routing decides *where*, never *when*.

## 8. Parked for later (v2)

The earlier draft's multi-repo model. Each session has its own repo, a TODO collects locations (repo + worktree) across sessions, the live agent can leave a `propose_session(repo, message)` hand-over, cards show several location marks and PR lines, and the timeline marks "moved to sb-web". Decided there and worth keeping: one worktree per repo per TODO, other repos read-only by contract, and the operator routing every new session, not only the first. The full text is in git history (commit 3a31b00).
