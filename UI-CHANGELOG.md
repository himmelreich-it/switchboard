# UI changelog

UI changes built without a Claude Design version. Feed the "Open" entries to Claude Design, then move them under the design version that covers them. Rules for entries: `CLAUDE.md`.

Baseline: design v9 (`designs/Switchboard v9.html`, 2026-10-13).

## Open

### 2026-10-15 · Open in VS Code button (bf3b18e, 31ce8be)
- Where: Focus header › topic popover.
- Now: a button "Open in VS Code" sits in the popover when the TODO has a worktree or a topic folder. It opens that folder in VS Code and closes the popover. Success shows the toast "Opened in VS Code · ~/code/worktrees/foo". Failure shows the error toast "Can't open VS Code" with a hint for the platform.
- Why: jump from a TODO into the editor without finding the folder by hand.
- Design question: does the button belong in the popover, or in the header next to the worktree chip? Icon or text?

### 2026-10-15 · Terminal tab click gives the terminal the keys (bde60cf, f349e0a)
- Where: Focus › Terminal tab.
- Now: a click on the Terminal tab, or opening a TODO that was left on that tab, puts the keyboard focus in the selected terminal. Each TODO remembers its selected terminal across page reloads.
- Why: you had to click into the terminal a second time before typing.
- Design question: behaviour only, nothing new is drawn. Should the focused terminal show a focus state?

### 2026-10-15 · Long schedule chip on a Scheduled card (f3ab4f4)
- Where: Board › Scheduled lane › card footer.
- Now: a long schedule chip, for example "Mon, Wed, Fri 09:00, 12:00, 17:00 · skipped", shrinks and ends in "…". Run now stays at the right edge, inside the card. The chip's tooltip shows the next run, not the full schedule.
- Why: a long chip pushed Run now out of the card.
- Design question: cut the chip with "…", or shorten the chip text itself, or wrap the footer onto two lines?

### 2026-10-14 · Open a removed TODO from the Removed list (452d5c2, 2e015ef)
- Where: Backlog › "Removed · n" list, and Focus.
- Now: each row in the Removed list highlights on hover and opens the TODO in Focus on click. Focus shows a read-only view:
  ```
  ←  T-42  Fix login redirect            [Removed 2h ago]  [Restore]  | 1/3 ▾
     Chat | Files
     ...chat history, no composer...
  ```
  The header shows the badge "Removed 2h ago" (tooltip "removed by …") and a primary Restore button in place of the status and ⋯ menu. Hidden: the composer, the Terminal tab, Start, the agent select and the Edit on the first message. Files still open and save.
- Why: you could not read a removed TODO's chat or files without restoring it.
- Design question: how should a removed TODO look in Focus (muted header, banner, other)? Should its files be editable?

### Unclear whether v9 covers these (built 2026-10-11/12, before v9 landed)
Check against v9's scenes; move to "Covered › v9" or keep open.
- Follow-up pills (1195c7c): with two pills on a card, the shorter one keeps its full width but leaves at least 70px for the longer one, which is cut with "…". Before, the longer pill could leave the card.
- Follow-up pills (6c927f2): pills show only on cards in a lane where the agent waits for the user. A TODO whose agent asked while background tasks still run sits in Waiting and shows its pills; a Running card never does.

## Covered

### v9 (2026-10-13)
- Scheduled lane, schedule chip and popover, Repeat row in New TODO, `from T-12` badge, Settings › Runs (c66edbc).
