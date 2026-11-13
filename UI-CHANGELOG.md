# UI changelog

UI changes built without a Claude Design version. Feed the "Open" entries to Claude Design, then move them under the design version that covers them. Rules for entries: `CLAUDE.md`.

Baseline: design v10 (`designs/Switchboard v10.html`, 2026-10-30).

## Open

### 2026-11-13 · Codex asks a question
- Where: Focus › chat, the request card above the composer; the TODO's card in Waiting on you; the Focus header badge.
- Now: when Codex asks the user a question with choices, the card above the composer reads "Codex asks · <question>", with the question's note under it, one numbered button per choice plus "Skip", and the line "Or answer in your own words: send a message." The composer hint reads "Sends as your answer to the question above." and its button "Answer". The work group line reads "Waiting for your answer". The board card shows "Codex asks" with the question and the first two choices as buttons, then "Reply →". The Focus header badge reads "Question" (cyan) instead of "Needs approval". Several questions come one card after the other.
- Why: Codex asks questions with choices in plan mode; before, it got no answer and went on guessing.
- Design question: should a question have its own icon instead of the shield of a permission request, and should several questions show at once as a form?

### 2026-11-13 · Codex plan mode in the session settings
- Where: Focus header › session popover, and the session chip.
- Now: a Codex TODO has a row "Collaboration mode" (Default, Plan) next to model and effort. When Plan is on, the chip reads "… · Plan · …".
- Why: Codex's plan mode was only reachable by typing /plan.
- Design question: should plan mode sit with the permission mode instead, as Claude's plan mode does?

### 2026-11-13 · Codex terminal
- Where: Focus › Terminal tab, the buttons under the terminal list, and the new-terminal form.
- Now: next to Shell and Claude a "Codex" button opens the same form as Claude's, headed "New Codex terminal", with the Codex command line under "Runs". The tab reads "Codex 1". The help line reads "A Claude or Codex terminal is its own session with your settings."; the empty panel "Open a shell or an agent terminal."
- Why: the user wanted the Codex CLI with the TODO's context and the app tools, as the Claude terminal has.
- Design question: with three buttons, should the agent terminals move into one "Agent" button with a choice?

### 2026-11-13 · API key login for Codex
- Where: Settings › Agents, the Codex row.
- Now: a password field "API key" sits before the "API Key" button. The button logs in with the key in the field; an empty field shows the toast "No API key · Paste the key into the field next to Codex's button first."
- Why: the API key login worked only with a key in the app's environment.
- Design question: should the field show only after a click on "API Key"?

### 2026-11-13 · Texts for Codex
- Where: Settings › Agents, Settings › Topics, Focus header, chat.
- Now: the Codex row in Settings › Agents reads "Uses your Codex login and ~/.codex config: skills, plugins, MCP servers and hooks." The topic folder hint reads "Agents launch here, so the folder's agent files apply: CLAUDE.md, CLAUDE.local.md and .claude/ for Claude, AGENTS.md and .codex/ for Codex." The Stop tooltip reads "like Esc in its own CLI". A Codex denial reads "Auto review blocked …". A Codex command that outlives its turn ends with the app message "Background command ended: <command>".
- Why: these texts named Claude where the agent can now be Codex.
- Design question: none.

### 2026-11-12 · Messages from another TODO's agent
- Where: Focus › chat, a message in the user's column.
- Now: an agent can send a message to another TODO when the user asks it to. The message shows in the receiving TODO's chat as the user's messages do, with the badge "From T-12" (click opens T-12), the same badge a TODO created by another TODO's agent has on its first message. Before, that badge only appeared on a first message.
- Why: the user wanted one TODO to be able to send a message to another TODO's chat.
- Design question: should a message from another TODO's agent sit in the user's column with "You" as its author, or get its own author label and color?

### 2026-11-10 · Preview for HTML files
- Where: Files tab, the mode switch of an HTML file (Preview · Read · Edit).
- Now: an HTML file opens in a new first mode "Preview", which shows the page as a browser draws it, on a white background, across the whole file area. Its scripts run, but cannot reach Switchboard. Read still shows the source with line numbers; Edit edits it. The preview reloads when the file changes on disk or after an edit. Find (⌘F) and comments are not offered in Preview.
- Why: agents write reports as HTML files, and the user wanted to see them rendered instead of as source.
- Design question: should Preview have its own control (e.g. an "Open in browser" button), and should it follow the dark theme or stay white like a browser?

### 2026-11-09 · Mermaid diagrams in rendered markdown files
- Where: Files tab, a markdown file in Read mode and in the Split and Preview views of the editor.
- Now: a code block marked as `mermaid` shows as a drawn diagram (flowchart, sequence diagram and the other Mermaid kinds), centered in the code block's box, with its source line in the gutter as before. The diagram takes dark or light colors from the theme and is drawn again when the theme changes. A block that Mermaid cannot read stays as code; hovering it shows Mermaid's error.
- Why: the user writes explanations with Mermaid diagrams and wanted to see them drawn, not as source.
- Design question: should a diagram that does not parse show its error visibly under the code instead of as a tooltip, and should the diagram box have its own background?

### 2026-11-05 · Login page in a browser on another machine
- Where: the whole page, when Switchboard is opened from another machine on the network and the browser is not logged in.
- Now: a centered card like the "This page needs a new link" card, titled "Log in to Switchboard · <machine>". It holds a button "Log in with a passkey" (only over https:// with a name, and once a passkey exists), then a password field with a "Log in" button. A wrong password shows "wrong password" in red under the form; five failures in a minute show "too many failed logins from <address>; try again in N s". Over plain http:// a small line reads "Not encrypted (http://): use this only on your own network. Passkeys need https:// and a name." Without any password set, the card says "No password is set. On <machine>, run bun server.ts --password, then reload this page." From a trusted address the card does not show at all; the page logs in by itself. A one-time passkey link shows the card "Add a passkey to Switchboard · <machine>" with a short explanation, a name field ("Name, e.g. 1Password") and a "Save a passkey" button.
- Why: the user wanted to use Switchboard from other machines in the browser, logging in with 1Password.
- Design question: should the login card show the machine's address too, so two machines with similar names are told apart?

### 2026-11-05 · Settings › Network
- Where: Settings, a new section "Network" between Runs and Appearance.
- Now: three cards. "On the network" lists the addresses other machines open, then one line: password set or not, HTTPS or "plain HTTP, not encrypted", the addresses that need no login, and the ranges it is open to. When it is off, the card says "Off: only this machine can open Switchboard" and how to turn it on; a broken setup file shows its error in amber. "Passkeys" lists each passkey with its name, its address, when it was added and last used, and a red Remove button; under it a name field and "Add passkey" when the page is open by an https:// name, otherwise a line explaining where to add one. "Logged-in browsers" lists each browser ("Chrome on Windows"), its address, "this browser" for the current one, how and when it logged in, last use, and a Log out button; under the list a red "Log out all browsers".
- Why: passkeys and logged-in browsers need a place to be seen and removed.
- Design question: should the address list offer a Copy button per row, or a QR code for opening it on a phone?

### 2026-11-05 · Machine name in the tab title, no VS Code buttons from another machine
- Where: the browser tab title; TODO header › topic popover (Open in VS Code); the places list (VS Code button); file view toolbar (Open folder).
- Now: opened from another machine, the tab title ends with the server's name, for example "(2) switchboard · desk", and the installed web app is named "Switchboard · desk". The VS Code and Open folder buttons are hidden there, since they would open on the server's screen.
- Why: with Switchboard open from several machines, the user must see which one a tab belongs to.
- Design question: should a remote page also show the machine name in the title bar, next to the logo?

### 2026-11-04 · Side-by-side diff
- Where: file view › Diff mode, header right side, next to the +N −M count.
- Now: a segmented control "Unified | Side by side" switches the diff layout. Side by side shows the merge-base version on the left and the current file on the right, each with its own line numbers and syntax colors, lines aligned across both panes. Removed lines have a red tint on the left, added lines a green tint on the right, and changed words inside a line a darker tint of the same color. A thin red or green bar in the gutter marks each changed line. Side by side is the default; the choice is remembered for all files and TODOs. ⌘F opens the editor's search bar on the right pane.
- Why: the user wanted VS Code-style side-by-side diffs.
- Design question: should Side by side collapse long runs of unchanged lines, as GitHub does, or keep the whole file like VS Code?

### 2026-11-04 · Reply in a comment's thread
- Where: Files tab › Comments column › comment card.
- Now: a comment holds a back-and-forth with the agent. Under the first comment the card lists each turn in order: the agent's replies in the inset box labeled "Agent", the user's answers below a muted "You" label. When the agent replied last, the card's buttons start with "Reply" and there is no Edit; Reply opens a box "Reply · ⌘Enter sends" with Cancel and Reply in place of the buttons. Sending puts the answer at the end of the thread and sets the comment back to Open, so it counts in "Ask agent · N" again. When the user wrote last, the card shows Edit instead of Reply, and Edit changes only that last text: the box sits under the thread so far. Resolved comments show no Reply. The delete dialog now reads "The comment and its replies are deleted."
- Why: the user often discusses a passage with the agent over several rounds, and editing the first comment lost the earlier exchange.
- Design question: should a long thread fold its older turns ("3 earlier replies") so the column stays scannable?

### 2026-11-03 · Theme button cycles Light, Dark, Auto
- Where: title bar, the theme icon button left of Settings.
- Now: the button shows the current mode instead of the one it switches to: a sun for Light, a moon for Dark, a half-filled circle for Auto. Each click steps Light › Dark › Auto › Light. The tooltip names the mode, the theme in use and the next step, for example "Auto · Catppuccin Mocha · click for Light".
- Why: the user wanted to reach Auto from the title bar, not only from Settings › Appearance.
- Design question: is a three-step cycle clear enough, or should the button open a small menu with the three modes?

### 2026-11-03 · Catppuccin themes
- Where: Settings › Appearance › Light theme and Dark theme tiles.
- Now: four new theme tiles in the official Catppuccin palette. "Catppuccin Latte" (Pale lavender-grey, mauve actions) sits next to Cyberpunk Light and Monokai Light. "Catppuccin Frappé" (Muted slate blue, pastel accents), "Catppuccin Macchiato" (Deep slate, pastel accents) and "Catppuccin Mocha" (Darkest flavor, pastel accents) sit next to Cyberpunk Dark and Monokai Dark, so the dark grid has five tiles over two rows. All four use Monokai's shape and type: Inter, sentence case, rounded corners, round state dots. Buttons are mauve, links and the focus ring lavender (blue links and a mauve ring in Latte). The page is the flavor's mantle, cards its base, inputs and terminals its crust. Waiting is yellow, done green, errors red. The hint under Mode names the flavor, for example "Always dark. Catppuccin Mocha is in use.", and the title-bar theme button's tooltip reads "Light theme · Catppuccin Latte".
- Why: the user wanted switchboard in the Catppuccin themes.
- Design question: should the Catppuccin flavors get their own group in the dark grid, with Frappé, Macchiato and Mocha in one row, instead of wrapping after the first three tiles?

### 2026-11-03 · Switchboard Beta app
- Where: the desktop app itself: its icon in the taskbar, Dock and Start menu, and a dialog at launch.
- Now: a second install, "Switchboard Beta", can sit next to the regular app. It uses the beta icon (the app icon with an amber "BETA" tag in the lower right corner) and shows the same TODOs. If the other app is already running, it does not open; an error dialog titled "Switchboard Beta" says "Switchboard is running on the same data. Quit it first, then start Switchboard Beta." (and the other way round). On Windows the app icon in the taskbar now fills its square like other apps instead of showing with a wide margin.
- Why: the user wanted to try a branch as a real app, with their real TODOs, without replacing the installed one.
- Design question: should the beta also say so inside the window, for example a "Beta" tag next to the app name in the title bar?

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
