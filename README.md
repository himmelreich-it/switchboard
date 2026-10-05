# switchboard

Local web app where every TODO is a Claude Code session: create TODOs across projects, start each one when ready,
steer it with short chat messages, and read or edit the markdown files the agents write. A pinned operator
session answers questions about all TODOs and acts only when told. Spec: `designs/feature-spec-switchboard.md`.

```sh
bun install
bun server.ts        # http://127.0.0.1:4477 (PORT to change)
bun run app          # same app in its own window (Electron); closing it stops running agents like Ctrl-C
bun test             # pure helpers in lib.ts
bun run install-local  # build Switchboard.app (no installer) and copy it to /Applications, for Spotlight and the Dock
```

`install-local` quits a running Switchboard first, which stops its running agents. The installed app runs a copy of
the code: run `install-local` again to update it. It still needs `bun` installed; it reads PATH and other variables
from your login shell (`zsh -ilc`), since apps started from the Dock or Spotlight do not get them.

Needs a Claude Code login. Agents run with your normal Claude Code settings, in auto permission mode.
Data lives in `~/.switchboard/` (`SWITCHBOARD_HOME` to change): `db.sqlite` and `data/<project-slug>/<todo-id>/`
for the agents' files. On first start, projects are imported from `~/.auto-coder/db.sqlite`.
