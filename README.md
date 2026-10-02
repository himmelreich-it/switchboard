# agent-coordinator

Local web app where every TODO is a Claude Code session: create TODOs across projects, start each one when ready,
steer it with short chat messages, and read or edit the markdown files the agents write. A pinned coordinator
session answers questions about all TODOs and acts only when told. Spec: `designs/feature-spec-agent-coordinator.md`.

```sh
bun install
bun server.ts        # http://127.0.0.1:4477 (PORT to change)
bun test             # pure helpers in lib.ts
```

Needs a Claude Code login. Agents run with your normal Claude Code settings, in auto permission mode.
Data lives in `~/.agent-coordinator/` (`AGENT_COORDINATOR_HOME` to change): `db.sqlite` and `data/<project-slug>/<todo-id>/`
for the agents' files. On first start, projects are imported from `~/.auto-coder/db.sqlite`.
