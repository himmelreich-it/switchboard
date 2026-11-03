# UI changelog

UI changes built without a Claude Design version. Feed the "Open" entries to Claude Design, then move them under the design version that covers them. Rules for entries: `CLAUDE.md`.

Baseline: design v10 (`designs/Switchboard v10.html`, 2026-10-30).

## Open

### 2026-11-03 · Start command on the token screen
- Where: the screen shown when the page has no valid launch token ("open the link the server printed").
- Now: it names the command `bun run serve` instead of `bun server.ts`, which no longer exists.
- Why: the server is now a Rust program started with `bun run serve`.
- Design question: none; only the command in the sentence changed.

### 2026-11-02 · Find in Read mode
- Where: Files tab › file view, bottom edge above the footer note.
- Now: ⌘F (or Ctrl+F) in Read mode, in the markdown Preview and in Changes opens a find bar, styled like the editor's search panel: a field "Find", a count "2 of 3" or "No results", up and down arrows "Previous match" and "Next match", and a close cross. Matches are marked half-strength amber, the current one solid amber with dark text, in every theme; the editor's own search uses the same colors now (its old tints were near invisible in Monokai Dark). Enter and Shift+Enter step, Esc closes. In Edit mode and in read-only code ⌘F still opens the editor's own search panel. The Read button's tooltip now reads "Read · ⌘E toggles, ⌘F finds".
- Why: the user wanted to search the documents they read, not only the ones they edit.
- Design question: should the bar sit at the bottom like the editor's panel, or float at the top right like a browser's find?

### 2026-11-02 · Full-screen file
- Where: Focus › Files tab › file strip, right side, between the Comments toggle and Copy path.
- Now: an icon button "Full screen" (maximize arrows) makes the open file fill the window. The title bar, the TODO header with its tabs, and the file list on the left are hidden. Only the file strip, the file (read, edit or diff) and the comments column stay. The button turns into "Exit full screen · Esc" (minimize arrows). Esc leaves full screen first; a second Esc closes the file. On macOS the file strip leaves room for the window buttons and drags the window. Full screen stays on for the next file until the user turns it off or reloads.
- Why: the user wanted to read a markdown file and its comments with nothing else on screen.
- Design question: is an icon button in the strip the right control, or should it have a shortcut and a label?

### 2026-10-30 · Archive a TODO file
- Where: Files tab › TODO root › file header, the icon row next to "Add the path to the message".
- Now: an agent's file has an archive icon "Archive file". A dialog "Archive file" says the file moves to the TODO's archive folder and leaves the list, with its comments; buttons Cancel and Archive. Success shows the toast "Archived <file>". While the agent runs the icon is disabled. The user's own notes keep "Delete note". A file deleted on disk now leaves the list by itself instead of showing "missing". The note under the list reads "terminal/, uploads/ and archive/ are not listed".
- Why: a TODO collected 180 files, 172 of them already deleted, and the user could not remove any.
- Design question: archive icon in the header, or a hover action on each row of the list?

### 2026-10-30 · Work files group
- Where: Files tab › TODO root › list, below the TODO's own files.
- Now: files the agents keep for themselves (logs, raw results, scripts) sit in one folded row "▸ Work files <count>", muted, closed by default. A click opens it and lists those files under it, same row style as the others.
- Why: agents wrote 160+ logs and result files next to the plans and reports, and the user could not find the files meant for them.
- Design question: should the group be indented or carry its own heading, and should the Files tab count include work files?

## Covered

### v10 (2026-10-30)
- Follow-up pills (1195c7c): with two pills on a card, the shorter one keeps its full width but leaves at least 70px for the longer one.
- Follow-up pills (6c927f2): pills show only on cards in a lane where the agent waits for the user.
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
