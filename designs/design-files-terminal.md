# Files, editing and terminals

Proposal, 2026-10-06. Extends `feature-spec-switchboard.md` with browsing the files of a TODO's worktree or its project's main folder, light editing in the style of Orca, and terminals per TODO: clean shells, or Claude Code started with the TODO's context. Items are numbered from 68, the next free spec number; once agreed they move into the spec, and the UI goes into `designs/Switchboard-v4.html` before it is built (the spec's Design rule).

Guiding principle, as in the spec: **streamline the user's work, don't delegate their control.** The chat stays the lightweight way to steer an agent. The terminals and the editor are for doing things yourself, or with a Claude session of your own, without leaving the app for an IDE.

## Replacing the Events tab with terminals

Yes. The Events tab is an audit trail (runs, hooks, attention changes with their actor, denials) that is opened rarely, mostly to find out why a TODO is in its state. A terminal is used often. So the Focus view tabs become **Chat · Files · Terminal**, and Events becomes a toggle in the Chat tab that mixes the events into the chat (item 68). Nothing is lost, and you can reply while the events are shown.

The Terminal tab holds as many terminals as you need, each in its own tab. A new terminal is one of two kinds:

- **Shell**: a clean shell in the worktree, for docker, git, tests or a dev server.
- **Claude**: a new Claude Code session in the worktree that starts with the TODO's context: what its agent is doing, its branch, the recent chat and its files.

The TODO's own session stays in the app and is only steered through the chat. Claude in a terminal is always a separate session.

## Functionality

### Focus view

68. The Focus view of a TODO has the tabs Chat, Files and Terminal. The Events tab (item 58) goes away, and the Chat tab gets an **Events** toggle with the TODO's event count.
    - **Off** (the default), the chat shows messages only, as today.
    - **On**, the TODO's events are mixed into the chat in time order. Each event is a dim one-line row with time, kind, detail and actor, for example `14:02 · PreToolUse · Bash · agent`. A long detail, such as a turn's full result text (item 48), opens on click.
    - The composer stays below in both modes, so the user can reply while reading the events.
    - New events appear live, like messages. The pick is kept in localStorage and applies to every TODO.
    - The operator's chat has no toggle and is unchanged.

### Files

69. The Files tab has a root switcher:
    - **TODO files**, the default: the artifact files of items 22 to 26, listed as today.
    - **Worktree**: the TODO's registered worktree, labelled with its branch. It is shown only while a worktree is registered and exists on disk.
    - **Project**: the project's local folder, which is the main checkout.

    The pick is remembered per TODO until the page is reloaded.
70. The Worktree and Project roots show a folder tree of the files git knows about: tracked files plus untracked files that are not ignored (`git ls-files --cached --others --exclude-standard`). Ignored files such as `node_modules` and build output are not shown. Folders expand on click.
71. The tree marks changed files as modified, added, untracked or deleted:
    - The marks combine uncommitted changes (`git --no-optional-locks status --porcelain=v2`) with the changes committed on the branch since its merge base with `origin/HEAD`, the same base the card's git line uses (item 5).
    - A **Changed** filter lists only these files, flat, with a count. Deleted files appear only there.
72. Opening a file shows it in the file view, the panel artifact files use today. The panel shows the path relative to the root, the size, and the Read and Edit modes.
    - Markdown files keep the rendered preview (item 52).
    - Other text files show syntax-highlighted source.
    - Images show as an image, served with `Content-Security-Policy: sandbox` as attachments are (item 62).
    - Binary files, and text files over 2 MB, show only their size.
73. A changed file also has a **Diff** mode: the merge-base version against the file on disk, unified, with changed lines highlighted. It is read-only.
74. The file view has **Copy path** and **Add to message**. Add to message inserts the path, relative to the worktree, into the TODO's message draft, so pointing the agent at a file takes one click.

### Editing

75. Edit mode uses a code editor (CodeMirror 6) for every file, including TODO files, and replaces the textarea. Markdown keeps its side-by-side preview. ⌘E and ⌘S work as today (item 59), and ⌘F searches within the file.
    - Editing is light on purpose: there is no creating, renaming or deleting of files, no search across files, no LSP and no git actions. The agent or the terminal does those.
76. Locking and conflicts:
    - **TODO files**: item 25, unchanged.
    - **Worktree files**: read-only while the TODO's agent is `running`, as in item 25. Read mode follows the file as it changes.
    - **Project files**: editable at any time. TODOs without a worktree may work in the main folder, and the app cannot tell which files they touch, so the conflict check is the guard here.
    - **Every save** sends the mtime the file was loaded with. If the file changed on disk since then, saving warns and offers Overwrite or Discard, as in item 25.
    - Saving does not notify the agent (item 24).
77. The tree and the open file refresh in two ways. While a Worktree or Project root is visible, git is polled every 5 seconds. The TODO's runs also trigger a refresh when they end.
    - An open file without unsaved edits reloads when it changes on disk.
    - There are no file-system watchers. A recursive watch on Linux takes one inotify watch per folder, including ignored ones like `node_modules`, and polling git behaves the same on every OS.
78. File requests name a root and a path relative to it.
    - The server resolves the real path and accepts only paths inside the project's local folder or its worktree root (item 47). Symlinks the worktree skill makes, such as `.env` or `CLAUDE.local.md` pointing into the main folder, therefore open, and nothing outside the project opens.
    - Writes go to the resolved file, so symlinks are kept.
    - Writes inside `.git/` are refused.

### Terminal

79. The Terminal tab holds any number of terminals for the TODO, each in its own tab with a × to close it. The Terminal tab's label shows how many are open.
    - A **+** button opens a new terminal, as a **Shell** or as **Claude** (item 81).
    - Both kinds start in the TODO's worktree if one is registered, else in the project's local folder. When there is a worktree, the + menu also offers both kinds in the project folder, for example for a git command on the main checkout.
80. A terminal tab is labelled with the title its program sets (shells and Claude Code set one). Without a title it reads `Shell 2` or `Claude 1`. A terminal in the project folder while a worktree exists also gets `· project`.
81. **Claude with context.** New Claude opens a short form with the folder and an editable first message, prefilled with:

    > I'm working on the Switchboard TODO "&lt;title&gt;". What its agent is doing, its branch and PR, the recent chat and its files are in `<context file>`. Read it, then wait for my instructions.

    The user can add their own ask, and ⌘↵ starts. The app then:
    - writes the context file (item 82);
    - runs `claude --add-dir <artifact folder> "<first message>"` in the chosen folder. The `--add-dir` lets Claude read and write the TODO's files without permission prompts.

    This is a new session, separate from the TODO's:
    - It runs with the user's normal Claude Code settings and permission mode, without the app's MCP tools or hooks.
    - Nothing it does appears in the TODO's chat, and the app does not store its session id. Claude Code keeps its transcript as for any session, so `claude --resume` in a shell finds it.
82. The context file is `<artifact folder>/terminal/context.md`, rewritten at each Claude launch. It is not listed under TODO files. It is a snapshot, taken at launch, of:
    - the TODO's title, project, status and first instruction;
    - the worktree path, branch, git line and PR line (item 5);
    - the whole chat, with author and time. Agent messages are one or two lines (item 18), so even a long chat stays small;
    - the TODO's files, with absolute paths, so Claude can read them;
    - the TODO's session id, so Claude can find the full transcript under `~/.claude/projects/` if it needs more history;
    - a note that the TODO's agent may be working in the same worktree at the same time, so Claude checks with the user before committing, switching branches or rewriting history.
83. A terminal's lifetime:
    - It keeps running when the user switches tabs or TODOs, closes the browser tab, or reloads. Reopening shows its recent output, then the live terminal.
    - It ends when its tab is closed (asking first while its process is alive), when the TODO is removed, or when the app shuts down (item 56).
    - When its process exits, the tab stays with the exit code and a **Restart** button; a Claude tab also gets **Shell here**.
    - Removing a TODO that has open terminals asks for confirmation and says how many will close, the same as item 12 does while the agent runs. Restoring a TODO does not bring its terminals back.
84. The programs:
    - **Shell**: on macOS and Linux, the user's login shell (`$SHELL -l`). On Windows, `pwsh` if installed, else Windows PowerShell. `SWITCHBOARD_SHELL` overrides the shell on every OS.
    - **Claude**: `claude`, found on PATH. In the window app, PATH comes from the login shell (item 66).
    - **Environment** for both: the server's, plus `TERM=xterm-256color` and `COLORTERM=truecolor`.
85. Keys and clipboard:
    - While a terminal has focus, every key goes to it, including Esc and N.
    - On macOS the ⌘ shortcuts keep working, because ⌘ never reaches a terminal.
    - On Windows and Linux, Ctrl combinations go to the terminal (Ctrl+J, Ctrl+E and Ctrl+S mean something there). Copy and paste are Ctrl+Shift+C and Ctrl+Shift+V.
    - Clicking outside the terminal hands the keys back to the app.
86. The TODO's agent does not see the terminals and is not told about anything done in them. Shells, Claude sessions and the agent can work in the same worktree at the same time, as in any terminal. Nothing guards against two of them running git at the same moment (as the spec's "Not built on purpose" does for shared worktrees). For Claude, the context file says so.
87. Files and terminals belong to a TODO. The overview, the operator and the Projects page have neither.

## How it is built

- **PTY in the Bun server.** `Bun.spawn(cmd, { terminal: { cols, rows, data, exit } })`, then `proc.terminal.write()` and `resize()`.
  - This needs Bun 1.3.5 on macOS and Linux and Bun 1.3.14 on Windows, where it uses ConPTY. The server checks `Bun.version` at start; with an older Bun, the Terminal tab names the version it needs.
  - node-pty is not an option: under Bun its PTY closes after the first output (tested with Bun 1.4.2), and putting it in Electron would leave browser mode without terminals.
  - A Claude terminal runs `claude` directly, with its arguments as an array, not through a shell. The first message therefore needs no quoting on any OS, but shell aliases for `claude` do not apply.
  - On Windows, `claude` can be a `.cmd` shim from an npm install, which has to run through `cmd /c`.
- **Terminals API.** Terminals are kept in the server's memory, not in the database.
  - `GET /api/todos/<id>/terminals` lists them as `{id, kind, cwd, exited, code}`.
  - `POST /api/todos/<id>/terminals` takes `{kind: "shell" | "claude", root: "worktree" | "project", message}` and returns the new terminal's id. `message` is only used for Claude.
  - `POST /api/todos/<id>/terminals/<tid>/close` ends one.
  - Opening, closing and exits reach other open pages as an SSE event (item 61).
- **One WebSocket per open terminal.** `GET /api/todos/<id>/terminals/<tid>/ws` is upgraded with `server.upgrade()`, using the `websocket` handler of `Bun.serve`.
  - Binary frames carry terminal bytes in both directions.
  - Text frames carry JSON control messages: `{type:"resize",cols,rows}` from the browser, `{type:"exit",code}` from the server.
- **Scrollback.** The server keeps the last 256 KB of each terminal's output.
  - On attach it sends a terminal reset, then that buffer, then live output. It then re-sends the size, so full-screen programs (vim, htop, Claude Code) redraw.
  - If replaying the raw buffer garbles too often, the fallback is Orca's approach: mirror each terminal into `@xterm/headless` on the server and send `addon-serialize` snapshots.
- **WebSocket security.**
  - The upgrade is accepted only when `Origin` is `http://127.0.0.1:<port>` or `http://localhost:<port>`, on top of today's Host check. This matters because browsers do not apply CORS to WebSockets. Any website can open `ws://127.0.0.1:4477`, and its Host header is then correct, so the Host and content-type checks alone would hand every website a shell.
  - The Origin check stops websites, but not programs. Any program that can reach `127.0.0.1:4477` and simply skips the Origin header gets in. Examples are another user's programs on a shared machine, or a Linux Docker container started with `--network host`. Today such a program can already drive the whole API (create and start TODOs); a terminal gives it a shell directly.
  - A **launch token** closes that gap (open question 1). It works like this:
    - At each start the server picks a random secret and keeps it only in memory.
    - Every API request and WebSocket must carry it in a cookie. Without it the server answers 401.
    - The window app sets the cookie before loading the page. In browser mode the server prints `http://127.0.0.1:4477/?token=…` once at start; the page moves the token into the cookie and drops it from the URL. Jupyter does the same.
    - A program that cannot see the secret, such as another user's program or a container, is shut out.
- **Files API.** Every request names a root (`worktree` or `project`) and a relative path, checked as in item 78.
  - `GET /api/todos/<id>/tree?root=` returns `{paths, changed}`.
  - `GET /api/todos/<id>/fs?root=&path=` returns `{content, mtime, size, binary}`.
  - `PUT /api/todos/<id>/fs` takes `{root, path, content, base_mtime, force}` and answers 409 on a conflict, like the existing file `PUT`.
  - `GET /api/todos/<id>/diff?root=&path=` returns `{base, current}`, with the base read by `git show <merge-base>:<path>`.
  - Each tree request runs two or three git processes of about 0.03 s each, the same kind of cost as `gitState()`.
- **Libraries.**
  - **Terminal:** xterm.js 6 (`@xterm/xterm` with `addon-fit`, `addon-webgl` and `addon-web-links`).
  - **Editor:** CodeMirror 6 (`codemirror` and `@codemirror/merge`, plus `@codemirror/lang-*` for markdown, JS/TS, JSON, CSS, HTML, Python, YAML and shell).
  - **Install and load:** both are installed with bun and bundled by `bun run vendor` into `vendor/terminal.js` and `vendor/editor.js`, which are minified, committed, and loaded by `index.html` on first use. `index.html` keeps no build step, the app works offline, and one copy of `@codemirror/state` is guaranteed; loading CodeMirror's packages separately from a CDN can produce two copies, which breaks its extensions.
  - **Size:** about 90 KB gzipped for xterm.js, and about 125 KB plus the languages for CodeMirror.
  - **Packaging:** the Electron `files` list gets `vendor/**`.
  - **Why not Monaco:** its ESM build needs a bundler that handles CSS imports, it is about 2.5 MB, and it is built for a full editor, which this is not.
- **Theming with the design system.**
  - CodeMirror's theme uses the CSS variables directly (`--surface-inset`, `--text-primary`, `--font-mono`, a lime cursor, `--tint-lime` selection), so Auto, Dark and Light need no extra code.
  - xterm.js needs color values instead. They are read with `getComputedStyle` at open time and again in `applyTheme()`. The ANSI colors map to the accent tokens: red to `--accent-red`, green to `--status-success`, yellow to `--accent-amber`, blue to `--accent-blue`, magenta to `--accent-violet`, cyan to `--accent-cyan`.
  - The terminal opens after `document.fonts.ready`, so it measures JetBrains Mono rather than the fallback font.
- **Windows and Linux.**
  - Nothing in this feature is macOS-only.
  - On Windows, ConPTY can reorder output, mouse tracking does not work on Windows 10, and closing a terminal is slow before Windows 11 24H2.
  - `loginShellEnv()` in `electron/main.js` is skipped on Windows.
  - Packaging for Windows and Linux (electron-builder `nsis` and `AppImage` targets) is a separate change; `install-local` is macOS-only today.

## Not built on purpose

- Driving the TODO's own session in a terminal (`claude --resume`). A Claude terminal is always a new session with the TODO's context. An app run cannot be shown in a terminal anyway, because the SDK runs Claude Code without its terminal UI.
- Refreshing a running Claude terminal's context. Its context is a snapshot from launch, and Claude can re-read the TODO's files and git state itself.
- Split panes.
- Files and terminals for a project without a TODO, for example on the Projects page.
- Terminals that survive quitting the app (a separate daemon as in Orca, or tmux as in Kanban Code). Quitting stops terminals, as it stops agents.
- Creating, renaming and deleting files; search across files; LSP; git actions (stage, commit, discard) from the UI.
- Showing ignored files.
- A browser preview pane (Orca and Kanban Code have one).
- Telling the agent about edits or terminal commands. The user says it in chat.

## Open questions

1. Should the app-wide launch token (see WebSocket security) ship with the terminals, or separately?

## Reference: how others build this

Researched 2026-10-06, from each project's source.

| | Shell and UI | Terminal | Files and editing | Git |
|---|---|---|---|---|
| [Warp](https://github.com/warpdotdev/warp) | Rust, its own GPU UI framework (Metal on macOS, wgpu on Windows and Linux) | Its own emulator adapted from alacritty_terminal, with a forked ConPTY on Windows | Its own Rust editor, tree-sitter, LSP | git2 |
| [Orca](https://github.com/stablyai/orca) | Electron, React 19, Tailwind, shadcn/ui | xterm.js 6 and node-pty in a detached daemon; @xterm/headless for reattach | A custom virtualized tree, @parcel/watcher, ripgrep, Monaco and its diff editor, no LSP | The `git` and `gh` CLIs |
| [Kanban Code](https://github.com/langwatch/kanban-code) | Native Swift on macOS, plus a separate Tauri and React port for Windows | SwiftTerm attached to tmux; on Windows xterm.js with tauri-pty | No tree or editor; "Open in Zed/Cursor/VS Code" | The `git` and `gh` CLIs |
| [Vibe Kanban](https://github.com/BloopAI/vibe-kanban) | A local Rust server, React in the browser, an optional Tauri wrapper | xterm.js with portable-pty for the user's shell; agents as stream-json processes | A diff viewer and a tree of changed files, no editor | The `git` CLI and libgit2 |

Switchboard sits closest to Vibe Kanban in shape: a local server, a browser UI, a thin desktop wrapper, and agents driven through a structured stream instead of a terminal. It takes its file and terminal features from Orca, using CodeMirror instead of Monaco and Bun's PTY instead of node-pty.
