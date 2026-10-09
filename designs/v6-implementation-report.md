# Switchboard v6: implementation report

Branch `design/v6`. Design: `designs/Switchboard v6.html`. Plan and inventories (outside the repo):
`~/.switchboard/data/switchboard/9/` (`v6-plan.md`, `design-inventory.md`, `current-inventory.md`).

## Done

- **index.html rewritten region by region** to the v6 layout. Plumbing kept: state, loaders, SSE, drafts,
  uploads, escaping and markdown sanitising, focus restore, scroll pinning, files, terminals.
  - **Title bar.** Board and Backlog tabs, Operator button (⌘K) with an unread square, theme toggle, gear.
    The Removed tab is gone.
  - **Operator side panel** (360 px). Markdown replies, tool line, `T-n →` link chips, input with draft,
    New session, session id and context.
  - **New TODO dialog** (N, ⌘↵, ⌘⇧↵). Keeps the project select, agent select and attachments.
  - **Board.** Repo filter select. Waiting cards answer in place (Allow, Reject, an options menu, an ask box
    with pills, a PR chip). Running cards show the activity line, plan segments, a queued chip, and the timer,
    which turns into Stop on hover. Idle shows dense rows on one subgrid: unread square, last word, pills,
    PR chip, and the time, which turns into Remove on hover.
  - **Backlog.** Rows with Start and Remove, plus a collapsible "Removed · n" list with Restore.
  - **Focus header.** Status cluster (Needs approval, Waiting × clear, Idle, Running m:ss + Stop, New session),
    a ⋯ menu, and the `1/3 ↓` waiting navigator. A chip row holds:
    - the repo and git popover, with the project move before the start;
    - the PR chip and its hover card;
    - the session chip `S3 Default · Opus · High`, whose popover shows per-TODO overrides with "This TODO ×"
      against the inherited value;
    - the context meter;
    - the tabs and the Transcript switch.
  - **Chat timeline.**
    - Old sessions fold to a summary line; the live session has a divider.
    - Messages: you as a bubble, agent as plain text, app as a card.
    - Each agent turn (prompt/launch → run ended) is a collapsible **work group** holding:
      - tool rows with Input, Diff (hunk viewer) and Output;
      - narration, thoughts and the plan checklist;
      - the live "Writing" stream and the Result card;
      - "Show N earlier steps" while the turn is live.
  - **Approval panel** docked above the composer, with numbered options and keys 1–9.
  - **Composer box.** Attach, delivery hint, ⌘↵, and a Send/Queue button. Pills sit above it, then the
    Deliver bar and the new-session row. The slash popup is restyled with groups, empty states and a footer.
  - **Files.** Left column with the root switch and either "TODO files" plus "Changed in worktree", or the
    tree. The file view is on the right.
  - **Terminal.** Info strip, left list with Shell and Claude buttons, the xterm pane, and the New Claude form.
  - **Settings.** Projects (list, Project card, Agent defaults card), Agents rows (Found / Not found, Add
    agent form), and Runs and Appearance (kept).
  - **Toasts.** Bottom right, one line, still stacking up to 3, with actions.
- **server.ts.** The TODO summary gains `run_started_at` (last prompt/launch), used for the timers.
- **Verified** against the design scenes in Playwright at 1440×900:
  - board, backlog with Removed, New TODO dialog, operator panel, settings and agents;
  - Focus with Bash approval and with Edit approval;
  - running with plan, idle with result, follow-up pills;
  - the session popover and the slash popup;
  - files and terminal;
  - light theme, and 960 px with the operator open.

  One real agent run in a scratch repo worked end to end: start, then a work group, then the reply.
  `bun test`: 26 pass.

## Left to do

1. **TODO numbers and operator routing** (`~/.switchboard/data/switchboard/3/v1-scope-and-todo-numbers.md`,
   all answers agreed).
   - The UI already shows `T-{id}`; the backend work is open:
     - TODO created without a repo;
     - the operator picks the repo at Start or asks (2–3 options);
     - "Change repo" while there is no worktree;
     - artifact folders move to `data/<todo-id>/` (one-time migration);
     - repo Profile (agent-written, Rescan);
     - workers get read-only `read_todo` / `read_file`;
     - `T-n` references are explained in the per-turn instructions;
     - item 35 is reworded.
   - The design branch `design/repos-and-session-locations` has the parked multi-repo doc.
2. **Spec update.** `designs/feature-spec-switchboard.md` still says v4. Point it at v6 and rewrite the UI items
   58, 121, 122, 86–93, 63, 124, 123, 125 and 59 (operator panel, New TODO dialog, Removed under the backlog,
   keys ⌘K, N and 1–9).
3. **Not yet live-tested:** the Electron window (title bar overlay on Windows). Tested since against a scripted
   ACP agent: Allow on the board card, keys 1 and 3 in Focus, ⌘J, N, the live "Writing" block, a message queued
   during a turn, the running timers, Esc in the operator panel.
4. **Open questions in `v6-plan.md`** got my defaults:
   - `T-{id}`;
   - "All repos" in the filter, "Projects" in Settings;
   - Removed folded under the backlog;
   - Auto theme kept in Settings › Appearance;
   - stacking toasts kept;
   - idle columns aligned.
5. **Polish.**
   - Old-session expand shows the summary plus the full read-only stream; the design shows only the summary.
   - Model and Effort "Agent's default (x)" have no value before a project default exists.
   - The code review and simplify pass ran (commits e4c3d4c, f4dc8e0). Left from it: 8 `<span title>` wrappers
     around buttons that could take a `title` option in `btn()`, three copies of the timer span markup, and two
     byte-size formatters (`fmtSize`, `fmtBytes`).
