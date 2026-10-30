# UI changelog

UI changes built without a Claude Design version. Feed the "Open" entries to Claude Design, then move them under the design version that covers them. Rules for entries: `CLAUDE.md`.

Baseline: design v10 (`designs/Switchboard v10.html`, 2026-10-30).

## Open

### Unclear whether v9 covers these (built 2026-10-11/12, before v9 landed)
Check against v9's scenes; move to "Covered › v9" or keep open.
- Follow-up pills (1195c7c): with two pills on a card, the shorter one keeps its full width but leaves at least 70px for the longer one, which is cut with "…". Before, the longer pill could leave the card.
- Follow-up pills (6c927f2): pills show only on cards in a lane where the agent waits for the user. A TODO whose agent asked while background tasks still run sits in Waiting and shows its pills; a Running card never does.

## Covered

### v10 (2026-10-30)
- 2026-10-27 · Idle shell tab closes without asking
- 2026-10-26 · A turn cut by sleep continues on its own
- 2026-10-26 · Change topic after a worktree exists; each topic keeps its worktree and PR
- 2026-10-23 · Auto effort in the session chip and popover
- 2026-10-22 · Running tab: agent background tasks and listening ports
- 2026-10-22 · Worktrees section in Settings
- 2026-10-21 · Web addresses in chat messages are links
- 2026-10-21 · New note button in TODO files
- 2026-10-20 · Line numbers in the markdown editor and the rendered markdown
- 2026-10-16 · Rendered markdown uses the full width; Comments column resizable
- 2026-10-16 · Comments on selected text in the Files tab
- 2026-10-16 · Independent mode switch in topic settings (dccc271, PR #4)
- 2026-10-15 · Open in VS Code button (bf3b18e, 31ce8be)
- 2026-10-15 · Terminal tab click gives the terminal the keys (bde60cf, f349e0a)
- 2026-10-15 · Long schedule chip on a Scheduled card (f3ab4f4)
- 2026-10-14 · Open a removed TODO from the Removed list (452d5c2, 2e015ef)

### v9 (2026-10-13)
- Scheduled lane, schedule chip and popover, Repeat row in New TODO, `from T-12` badge, Settings › Runs (c66edbc).
