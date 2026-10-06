# Backlog — design

Status: proposal, 2026-10-06. Once agreed, the items below go into `feature-spec-switchboard.md` (new items take the next free numbers, 68 onward) and the changed items there are updated.

## Problem

Every new TODO lands in the Not started lane on the same page as the work that needs attention. Capturing an idea therefore adds noise to the board, and a long list of ideas pushes the lanes that matter (Waiting on you, Running) apart. The goal is to make capturing ideas cheap, keep them out of the way, and let the user browse them and start one when they have the capacity for it.

## Idea in one line

The TODOs screen gets two views, **Board** and **Backlog**. The backlog holds every TODO that has not started; the board holds everything that has. Starting a TODO moves it from the backlog to the board, one way.

## Decisions

| Question | Decision | Why |
|---|---|---|
| What is the backlog? | All live TODOs with status `not_started`. No new status, no new table. | `not started yet` already means "an idea that has not launched" (items 6, 8, 11). |
| Order | Recently touched first (`updated_at`), as in the board lanes (item 40). | No schema change. Adding a note or editing an idea moves it to the top, which is a natural way to bump it. |
| Where to add | Only in the backlog. The New TODO card is part of the backlog view, with **Add** and **Add & start**. | The board shows started work only; a new TODO is not started, so adding is a backlog action. Add & start covers work that should begin right away. |
| Editing before start | The first instruction and the project can be edited while the TODO has not started. | Ideas are rough when captured and get refined before they are worth starting. |
| Back to the backlog | No. A started TODO never returns to the backlog. | Keeps item 11 (no pause). `idle` stays the resting state of started work. |

## Functionality

### Board and backlog

68. The TODOs screen has a **Board / Backlog** switch at the top, each side with its count. `B` toggles it. The pick is kept in localStorage, like the project filter. Below the switch, the board shows the pinned operator card, the project filter row and the lanes; the backlog shows the New TODO card next to the operator card, the project filter row and the backlog list. The board has no New TODO card.
69. The **board** shows the lanes `Waiting on you`, `Running` and `Idle`, in that order (item 40 without the Not started lane). With no started TODOs it says so and names how many are in the backlog, with a button that switches to it.
70. The **backlog** lists every TODO that has not started, most recently touched first, as compact rows rather than cards, so many ideas fit on one screen. A row shows the title (the first instruction until the TODO starts, clamped to two lines), the project, how long ago it was created, the number of notes and attachments waiting for the start, and a Draft badge. Each row has **Start** and **Remove** (no confirmation, as for any TODO that is not running, item 12). Clicking the row opens the Focus view.
71. Starting a TODO from the backlog leaves the user in the backlog, so they can keep browsing; a toast says it started, with a button to open it. Starting from the Focus view stays in the Focus view, as today.
72. The project filter (item 63) applies to both views. Its chip counts and the "in other projects" line count the TODOs of the current view. The amber square still marks projects with TODOs waiting on you in both views, because waiting is the signal that matters wherever the user is.
73. The Focus view's back button returns to the view the user came from and is labelled after it (`← Board`, `← Backlog`). `⌘J`, the "1 of 2 waiting" counter and the tab title count are unchanged: they only concern `waiting on you`, which only exists on the board.

```
 TODOs ─────────────────────────────────────────────────────────────
 ( Board 9 | Backlog 23 )                                       B
 [ New TODO ………………………………… ]  [ Operator ……………………………… ]
 Project  [All 23] [infra 11] [web 8] [docs 4]
 ───────────────────────────────────────────────────────────────────
 infra · 3d   Rotate the staging DB credentials and update…   Draft
              2 notes · 1 attachment                    [Start] [×]
 web · 5h     Try server-sent events instead of polling for…
                                                        [Start] [×]
```

### Adding

74. TODOs are added only in the backlog view. The New TODO card has **Add** (`⌘↵`) and **Add & start** (`⌘⇧↵`). Add creates the TODO as `not started yet`, as today (item 6), at the top of the backlog; the card's note reads "Lands in the backlog. Nothing launches until you start it." Add & start creates it and starts it in one request, so it leaves the backlog and appears on the board as `running`, with the same toast as item 71. Both record their events (`created`, then the start's run events) as separate actions would.
75. `N` focuses the New TODO input (item 59). On the board it switches to the backlog first.

### Editing before start

76. While a TODO has not started, its Focus view lets the user edit the first instruction and change the project. The first-instruction message in the chat has an **Edit** action that turns it into a textarea (`⌘↵` saves, `Esc` cancels), and the Project field in the Focus header becomes a select. Both are gone once the TODO starts.
77. Saving a new first instruction updates that message's body and the TODO's title, because the title is the first instruction until the agent sets one (item 7). It also counts as a touch and moves the TODO to the top of the backlog.
78. Changing the project moves the TODO's artifact folder from `data/<old-slug>/<id>/` to `data/<new-slug>/<id>/` and rewrites the stored attachment paths, so the first instruction's attachments go with it. Nothing else is stored per project before the start: no session, worktree or registered files exist yet.
79. The server refuses an edit once the TODO has started (409), for example after a start from another browser tab. The browser then keeps the unsaved text in the textarea and shows the refusal.
80. Attachments of the first instruction are not edited. More files and context go in as notes (messages that wait for the start, item 20), as today.

### Operator

81. TODOs the operator creates land in the backlog (item 36 unchanged in substance). The operator's system prompt (`runner.ts`) says "backlog" instead of "not started yet", so its answers match what the user sees. `list_sessions` needs no change: the status already tells backlog from board. The operator gets no tool to edit or start backlog TODOs (item 35).

## How it is built

- **Data.** No schema change. Backlog = `removed_at IS NULL AND status = 'not_started'`. Order is `updated_at DESC`, which `addMessage` already bumps on a new note.
- **Server.**
  - `POST /api/todos` takes an optional `start: true` and calls `start(id)` after `createTodo`.
  - New `POST /api/todos/:id/edit` with `{ instruction?, project_id? }`, following the existing action routes. It runs in one transaction: check `not_started`, update the first message and title, and on a project change rename the artifact folder and rewrite `attachments.path`. It writes an `edited` event with actor `user`.
- **Frontend.** `S.board` (`"board"` or `"backlog"`) next to the project filter in localStorage; `renderOverview` renders either the lanes (without `not_started`) or the backlog list; `LANES` loses its Not started entry. The New TODO card renders only in the backlog view and gets the second button and shortcut. `renderFocusHead` and the first chat message get the edit controls for `not_started`.
- **Spec items that change.** 5 (one page → two views), 6 (added only in the backlog, Add & start), 36 (wording), 40 (board lanes), 58 (screens), 59 (`B`, `⌘⇧↵`, `N` switches to the backlog), 63 (filter counts per view).
- **Design.** The spec says the UI follows the newest design, and `Switchboard-v3.html` has no backlog. Either a v4 design is made from this document before building, or the spec notes that the backlog screens follow this document until v4 exists.

## Not in scope

- Moving a started TODO back to the backlog, or any pause or shelving (item 11).
- Manual ordering, priorities, tags or due dates. If recently-touched turns out not to be enough, a rank column with drag-and-drop is the next step.
- Text search in the backlog. The project filter is the only narrowing for now.
- Editing or removing attachments or notes after they are committed.
- Starting several backlog TODOs at once.
- Operator tools to edit, reorder or start backlog TODOs.

## Open questions

- Should the Board / Backlog switch replace the header's single `TODOs` count with two counts, or stay inside the screen only? This document keeps the header as it is.
- Design v4 first, or build from this document and update the design afterwards?
