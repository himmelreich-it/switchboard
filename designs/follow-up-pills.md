# Suggested follow-ups (pills)

Status: RESOLVED · 2026-10-09 · branch `design/follow-up-pills`

## Intent

What the user said: after the agent reports back, have the operator or the working agent suggest two logical
follow-ups, shown as clickable pills; one click sends the pill's text to the agent.

> Review this PR → agent: "Looking good, no blocking issues." → pill **Approve the PR** → click → done.

Success: the common next step costs one click instead of typing, without the user losing control (nothing goes to
the agent until the user clicks) and without the app growing a second "brain" that guesses.

Assumptions (not said, decided here):

- A click is a normal committed user message (item 20): same delivery, same queueing, same history. So "Approve the
  PR" counts as the explicit go-ahead the contract requires for outward actions, because the user chose it.
- Suggestions are optional. No obvious next step, no pills.
- Worker TODOs only: the operator's own chat gets no pills in v1.

## Design

### Who writes them

The worker agent, through a new app tool. It has the full context of the turn and costs nothing extra. The operator
does not write them (item 35: it acts only on the user's instruction), and there is no separate model call.

```
suggest_follow_ups({ items: string[] })   // 1–2 items, each one line, ≤ 80 chars
```

- Written in the user's voice, as an instruction to the agent: "Approve the PR", "Post the two nits as review
  comments", "Use option B". Not questions, not descriptions.
- A later call in the same turn replaces the earlier set.
- Errors: empty list, more than 2 items, an empty or multi-line item, or an item over 80 chars → `isError` with the
  rule, like `post_message` does today.
- Registered for the TODO's agent only, not for Claude terminals (grant with `via`): a terminal has no chat turn to
  follow up on.
- Contract line (appended to `CONTRACT` in runner.ts):
  `- When your turn ends with obvious next steps, call mcp__app__suggest_follow_ups with up to two of them, phrased as the instruction the user would send you (e.g. "Approve the PR"). When you asked the user a question, the suggestions can be its likely answers. The user sends one with a click.`

### Lifetime

Stored in `todos.suggestions` (JSON array, null when none), so they survive a reload and an app restart.

| Moment | Effect |
|---|---|
| Agent calls the tool | `todos.suggestions` = items; `suggest` event by `agent` with the items joined by ` · ` |
| A turn starts (deliver, start, new session) | cleared: suggestions belong to the turn that wrote them |
| User commits any message (typed or pill) | cleared |
| Agent still `running`, or a permission request is open | kept but not shown |
| New session, remove | cleared |

Clearing at turn start means a turn that calls nothing leaves no stale pills from an earlier turn.

### Click

- Click: commits the pill's text as a user message, exactly as if typed and sent with ⌘↵ (`POST /api/todos/:id/messages`
  with `{ body, suggested: true }`). The server records a `suggestion picked` event by `user` and clears the set. The
  message itself is a plain `user` message; no new author.
- ⇧-click: puts the text into the composer instead (appended on a new line when there is a draft) for editing first.
  Tooltip: "Send to the agent · ⇧-click to edit first".
- A draft in the composer is left alone by a plain click.

### Where they show

Focus › Chat, between the last chat row and the composer, only when the TODO is not running and has no open
permission request:

```
  AGENT   Looks good: no blocking issues, two nits in review.md.   [review.md]
  ───────────────────────────────────────────────────────────────────────────
  ↳ Approve the PR     ↳ Post the two nits as review comments
  ┌───────────────────────────────────────────────────────────┐ [📎] [Commit]
  │ One line to the agent                                     │
  └───────────────────────────────────────────────────────────┘
```

Pill style: outlined, cyan (agent colour), `↳` prefix, wraps to a second line when both do not fit; text cut with an
ellipsis at the pill's max width, full text in the tooltip. Keyboard: Tab reaches them; no global shortcut.

Board card, under the card's last messages, under the same conditions (not running, no open permission request), so
the common case needs no Focus view:

```
  ┌ Review PR #212 ─────────────── Waiting on you ┐
  │ AGENT Looks good, no blocking issues.          │
  │ ↳ Approve the PR  ↳ Post the two nits          │
  └────────────────────────────────────────────────┘
```

On the card a click sends, as in the chat, and stops the click from opening the TODO. ⇧-click opens the TODO's Focus
view with the text in the composer. The pills take one row and are cut with an ellipsis to fit the card's width.

### Server and data

- `lib.ts migrateDb`: `addColumn("todos", "suggestions", "TEXT")`.
- `apptools.ts`: the tool above, worker grant without `via`. Add it to `WORKER_TOOLS` (the agent's allowlist); the
  Claude terminal launch in server.ts passes `WORKER_TOOLS` to `--allowedTools`, so it filters this one out there.
- `runner.ts`: clear `suggestions` at every run start (deliver, start, new session, and the background-task
  follow-up run of item 136, which starts without a delivery) and in `commitMessage`; contract line.
- `server.ts`: the messages route accepts `suggested` and adds the event; `summaries` returns `suggestions` so the
  UI gets them with the TODO.
- `index.html`: pills in `renderChat` and on the board card, click and ⇧-click handlers (`data-act="suggestion"`).
- Spec: new item 137 in `designs/feature-spec-switchboard.md` (Chat), and the tool in item 135's list stays without
  it (terminals do not get it).

### Testing

- `lib.test.ts`: the item validation (count, empty, multi-line, length) as a pure helper in `lib.ts`.
- Manual run: a TODO asked to "say hello and suggest two follow-ups" shows two pills; click sends and clears them;
  ⇧-click fills the composer; reload keeps them; a second turn without the tool shows none.

### Not in v1

- A suggestion that does something other than send text (approve via `gh` directly from the app, start a new
  session). The agent does the work after the click, as with any message.
- More than two pills.
- Suggestions from a separate model call (a fallback for agents that ignore the contract line; add if needed).
- Pills in the operator's own chat (would be the same tool on the operator grant, stored in `meta`).

## Open Questions

| ID | Question | Options | Recommendation | Status |
|----|----------|---------|----------------|--------|
| Q1 | Who writes the suggestions | A worker agent / B operator / C separate model call | A | RESOLVED |
| Q2 | Pills on the board card too | A card too / B Focus only | A | RESOLVED |
| Q3 | Operator's own chat gets pills too | A not in v1 / B yes | A | RESOLVED |

## Resolved Decisions

- Q1: the worker agent writes them with `suggest_follow_ups`. Accepted recommendation: full context, no extra cost, keeps item 35.
- Q2: pills on the board card too. Accepted recommendation: "click and done" from the board.
- Q3: no pills in the operator's own chat in v1. Accepted recommendation.
