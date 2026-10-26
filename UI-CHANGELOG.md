# UI changelog

UI changes built without a Claude Design version. Feed the "Open" entries to Claude Design, then move them under the design version that covers them. Rules for entries: `CLAUDE.md`.

Baseline: design v9 (`designs/Switchboard v9.html`, 2026-10-13).

## Open

### 2026-10-26 · Change topic after a worktree exists
- Where: Focus header › topic popover, the Change topic select.
- Now: the select also shows for a TODO that already has a worktree (before, the popover read "The topic is fixed once a worktree exists."). Its note then reads "Starts a new session in the other folder. The worktree stays on disk." Picking another topic drops the worktree and the PR from the TODO; the chat line says so: "Topic changed from A to B (worktree <path> stays on disk; PR #12 is no longer tracked). Your next message starts session 3 there."
- Why: the user wanted to move a TODO to another topic at any point, not only before a worktree exists.
- Design question: should picking a topic for a TODO with a worktree or PR ask first, in a dialog, since the TODO loses its branch and PR chips?

### 2026-10-23 · Auto effort in the session chip and popover
- Where: Focus header › session chip, and its Session settings popover, Effort row.
- Now: for a Claude TODO where neither the user nor the topic set an effort, the session chip shows the effort as "High (auto)" (medium until a pick). In the popover the Effort row's first choice reads "Auto (High)", its source label on the right reads "Auto", and a small line under the row gives the reason ("verifying", "picked at the start"). The level changes by itself: routing picks it at the start, and the agent switches it as the work changes (low to build, high to verify). Picking a level in the row overrides auto, as before, and "This TODO ×" goes back to auto. Each switch shows as an "effort" event line in the transcript, with the level and the reason.
- Why: the user wanted Claude's effort to follow the kind of work, after claude.dev/blog/spending-your-effort, without picking it by hand.
- Design question: the chip now reads "S1 Auto · High (auto)", with the permission mode's "Auto" next to the effort's "(auto)". Is that clear, or should the effort get a label ("Effort High") or an icon? Should an effort switch show in the chat itself, not only in the transcript?

### 2026-10-22 · Running tab: agent background tasks and listening ports
- Where: title bar, a third tab "Running n" after Backlog; its own screen.
- Now: the tab counts the background tasks the agents run, across all TODOs. Its screen has two sections. "Agent tasks" lists each background task: TODO number, TODO title, the task's name and its age ("12m"); a click opens the TODO; no stop button, since the user asks the agent. "Ports" has a Scan button; after a scan it lists every process and Docker container that listens on a port: the ports (":3000"), the process name or container name with a "Docker" badge, the full command in small mono text, its folder, the TODO it belongs to (a "T-42" chip that opens it) or its topic name or "—", its age, and buttons "Ask T-42" and "Stop". The header says "scanned 10:41" and "Docker: not running" when Docker did not answer. Stop asks in a dialog ("Stop", what it ends, Cancel / Stop) and then shows the toast "Stopped". Ask T-42 opens that TODO with "Stop the service on port 3000 (`npm run dev`)." in its message box. Switchboard's own row shows "Switchboard" instead of Stop.
- Why: the user wanted to see which background commands and services run, per TODO, and stop a leftover dev server or container directly or by asking the agent.
- Design question: is a third tab the right home, or should this be a panel on the board? With 30 listening processes (editor helpers, music app) the list is long: should it hide processes that belong to no TODO or topic behind a "Show all" toggle? Rows of Switchboard's own agent processes (name "claude") have a plain Stop today: should they read "agent" instead, like Switchboard's own row?

### 2026-10-22 · Worktrees section in Settings
- Where: Settings › a new nav entry "Worktrees" (with a count after a scan) between Agents and Runs.
- Now: the section is headed "Worktrees" with "Every git worktree of every topic, and every local branch without one, sorted by what can go. A scan only reads; Delete asks first." On the right: "Scanned 3m ago", a Scan button (Scanning… while it runs, then Rescan) and a red button "Delete 27 · 2.0 GB" (the ticked rows and their size; disabled with none ticked). The result shows three groups, each led by a badge and "<n> worktrees, <n> branches · <size> · <hint>": "Can remove" (green, "PR merged or closed, no TODO works in it, and nothing exists only here."), "Check first" (amber, "Changed files, commits on no remote, or no PR. Open the lists below to see what. Deleting loses them.") and "Keep" (grey, "A live TODO works in it, or its PR is open."). Each Can remove and Check first row starts with a checkbox; a scan ticks every Can remove row. A row: the folder name (or the branch name for a branch without worktree) in bold with the topic in small caps, a mono line "branch · 548 MB · last commit 2d ago" (or "branch without worktree · last commit 2d ago"), and a line of reasons such as "PR #591 merged · T-108 removed" or "no PR · no TODO · 1 changed file · 7 commits on no remote"; the PR number is a link. Rows with changed files or local-only commits have fold-outs "Changed files" and "Commits on no remote". Delete opens the dialog "Delete worktrees": "Deletes 17 worktrees with their branches and 10 branches without worktree. Remote branches and PRs stay. This can't be undone.", with a sentence about lost changed files and commits when Check first rows are ticked, the list of names (Check first ones with "⚠" and their reasons), Cancel and "Delete 27". Then the toast "Deleted 27" / "2.0 GB freed", and "Not deleted: n" with the reason per name for any the app refused.
- Why: worktrees took 7.5 GB, and the user wanted one list of what can go and what must stay across all topics, and to delete from it.
- Design question: does this belong in Settings, or on its own screen? Should Check first rows ask once more per row instead of one shared dialog?

### 2026-10-21 · Web addresses in chat messages are links
- Where: Focus › Chat, the agent's messages and the user's own messages.
- Now: a web address starting with http:// or https:// in a chat message shows in the link color and opens in the browser on click. A full stop, comma or closing bracket right after the address stays plain text. The rest of the message is unchanged plain text.
- Why: the agent posts addresses such as a local test server with its access token, and the user had to select and copy them by hand.
- Design question: should links also get a copy icon next to them, for addresses the user wants to paste elsewhere?

### 2026-10-21 · New note button in TODO files
- Where: Focus › Files tab, the "TODO files" heading, right side.
- Now: a small button "New note" creates an empty markdown note (notes.md, then notes-2.md, …) in the TODO's folder and opens it in Edit. The user's notes stay editable while the agent runs: no "Read-only · agent running" badge and no lock note for them. Their tooltip in the list reads "your note · editable while the agent runs". An open note has a trash icon button "Delete note" at the right end of the file header; it asks first (dialog "Delete note": "<name> is deleted from disk, with its comments. This can't be undone.", Cancel / Delete) and then shows the toast "Deleted". Agent files have no delete button.
- Why: the user wanted to write down thoughts while an agent works, and point the agent to them when it is ready.
- Design question: should the user pick the note's name when creating it? Should the user's notes look different from agent files in the list (an icon or a "you" tag)?

### 2026-10-20 · Line numbers in the markdown editor and the rendered markdown
- Where: Focus › Files tab, a markdown file in Edit (Write, Split, Preview) and in Read view.
- Now: the markdown editor shows a line-number gutter, like the code editor already does; the current line's number is brighter. The rendered markdown has a narrow gutter on the left: each heading, paragraph, list item, code block, table, quote and rule shows the source line it starts on, in small muted mono digits on the block's first line. Lines inside a code block and raw HTML blocks get no number.
- Why: the user wanted to point at and find lines in a file in both views.
- Design question: should the rendered view's numbers be always on, or behind a toggle (and hidden in Read view)? Is a 40-pixel gutter the right width for files with four-digit line numbers?

### 2026-10-16 · Rendered markdown uses the full width; Comments column resizable
- Where: Focus › Files tab, a markdown file in Read view or in the preview; the Comments column right of the file.
- Now: the rendered markdown fills the width of its pane instead of stopping at about 760 pixels. Tables fit that width: a long code value in a cell, such as a path or a URL, wraps inside its cell, so the table no longer scrolls sideways. A drag bar sits between the file and the Comments column, like the other splitters: drag to resize (200 pixels up to 60% of the file area), double-click to reset to 280 pixels, arrow keys move it. The width is kept across reloads.
- Why: wide tables in plans were cut off and needed a sideways scroll, and long comments were hard to read in the fixed narrow column.
- Design question: should long prose keep a maximum line length (centered or left-aligned) while tables use the full width?

### 2026-10-16 · Comments on selected text in the Files tab
- Where: Focus › Files tab, any open file (Read view, markdown preview, editor); a new Comments column on the right of the file; the file list.
- Now: selecting text in a file shows a small "Comment" button next to the selection. It opens a box at the top of a Comments column right of the file ("Comment · ⌘Enter saves"; Cancel, Comment). The commented text gets an amber highlight. Each comment card shows the selected text in small mono type with an amber bar, a state badge (Open amber, Replied cyan, Resolved grey), "line N", the comment, the agent's reply in a darker box labelled "Agent", and the buttons Edit, Resolve (Reopen when resolved) and Delete (asks first). When the text is gone from the file the card shows "Outdated" and the quote struck through. Clicking a card or highlighted text outlines the card and underlines the text. The column header reads "Comments · N" with a button "Ask agent · N" that puts "Look at my comments." in the chat box and switches to Chat. Resolved comments hide behind a link "Show resolved (N)". Files with open comments get an amber dot in the file list.
- Why: the user wanted to comment on parts of a plan or a code file and later have the agent apply each comment or answer it.
- Design question: should the Comments column be collapsible, or open only from a counter in the file header? Where does the "Comment" button sit best: next to the selection, or in the file header?

### 2026-10-16 · Independent mode switch in topic settings (dccc271, PR #4)
- Where: Settings › Topics › topic form, under Topic prompt.
- Now: a switch "Independent mode", off by default, with the hint "The agent pushes, opens and squash-merges its pull request once it trusts its work, and picks the recommended design option when confident. It still asks before anything destructive or outside this repository." Turning it on marks the form unsaved.
- Why: let the agent ship routine work in a topic without waiting for the user at each outward step.
- Design question: should a topic in Independent mode show it elsewhere, for example a badge on its TODO cards or in the Focus header?

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
