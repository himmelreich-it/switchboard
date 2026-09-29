# auto-coder

Local web UI that reviews GitHub PRs with Claude. See DESIGN.md.

```sh
bun install
bun server.ts        # http://127.0.0.1:4477 (PORT to change)
bun test             # pure helpers in lib.ts
```

Needs `gh` logged in and a Claude Code login. Data lives in `~/.auto-coder/db.sqlite`.
