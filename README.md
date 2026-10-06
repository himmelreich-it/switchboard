# switchboard

Local web app where every TODO runs as one or more Claude Code sessions: capture TODOs across projects in a backlog,
start each one when ready, steer it with short chat messages, start a fresh session when a step is done, browse and
edit the files of its worktree, and open terminals in it. A pinned operator session answers questions about all
TODOs and acts only when told. Spec: `designs/feature-spec-switchboard.md`.

```sh
bun install
bun server.ts        # prints http://127.0.0.1:4477/#token=… (PORT to change); open that link
bun run app          # same app in its own window (Electron); closing it stops running agents and terminals like Ctrl-C
bun test             # pure helpers in lib.ts
bun run vendor       # rebuild vendor/ (CodeMirror, xterm.js, marked, DOMPurify, fonts) after changing one of them
bun run install-local  # build Switchboard.app (no installer) and copy it to /Applications, for Spotlight and the Dock
```

The API needs the launch token the server picks at each start. `bun server.ts` prints a link with it, and the window
app reads it from `~/.switchboard/token`. After a server restart, open the new link. Everything the page loads is in
`vendor/`, so the app works offline. Terminals need Bun 1.3.5 or newer and are not available on Windows yet.

`install-local` quits a running Switchboard first, which stops its running agents and terminals. The installed app
runs a copy of the code: run `install-local` again to update it. It still needs `bun` installed; it reads PATH and
other variables from your login shell (`zsh -ilc`), since apps started from the Dock or Spotlight do not get them.

Needs a Claude Code login. Agents run with your normal Claude Code settings, in auto permission mode. Projects are
set up in Settings › Projects (the gear, or ⌘,).
Data lives in `~/.switchboard/` (`SWITCHBOARD_HOME` to change): `db.sqlite`, `token` while the server runs, and
`data/<project-slug>/<todo-id>/` for the agents' files. On first start, projects are imported from
`~/.auto-coder/db.sqlite`.
