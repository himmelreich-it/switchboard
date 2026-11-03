# The server, and the TypeScript it was ported from

The Rust crate in `core/` replaced `server.ts`, `runner.ts`, `apptools.ts`, `acp.ts`, `agents.ts` and `lib.ts`
(their last version is in commit 28d3285). It serves the same HTTP API, live event stream and terminal WebSockets to
the unchanged `index.html`. The port was faithful: same behaviour, same JSON field names and shapes, same SQL, same
event and message texts, same comments. Differences Rust forced are marked `// PORT:` in the code.

## Modules

| Rust module | Ported from |
|---|---|
| `db.rs`, `util.rs` | bun:sqlite, node:fs/path, `sh()` |
| `helpers.rs` | `lib.ts` (+ `lib.test.ts` as `#[cfg(test)]` tests) |
| `server.rs` | `server.ts` storage, live updates, git cache, read models (`summaries`) |
| `server_ops.rs` | `server.ts` PRs, worktree scan, ports, shared writes (`createTodo`, `removeTodo`, `setSchedule`), migration from auto-coder |
| `http.rs` | `server.ts` HTTP: `api()` routes, static files, SSE, the Bun.serve fetch handler |
| `terms.rs` | `server.ts` terminals + the PTY host |
| `acp.rs` | `acp.ts` |
| `agents.rs` | `agents.ts` |
| `runner.rs` | `runner.ts` |
| `apptools.rs` | `apptools.ts` (MCP over HTTP, no SDK) |
| `main.rs` | `server.ts` startup and shutdown |

Names: TS `camelCase` → Rust `snake_case`, in the module of the TS file it came from. Constants stay
`SCREAMING_CASE`.

## Conventions

- **Data is `serde_json::Value`** wherever the TS code uses rows, request bodies, agent messages or `any`. Use the
  `util::J` trait: `v.s("title")`, `v.i("id")`, `v.truthy("attention")`, `v.parsed("plan")` (a TEXT column holding
  JSON). Build objects with `json!`. Field order follows the TS object literal (serde_json keeps insertion order).
- **Types in signatures**:
  - strings: `&str` in, `String` out; nullable: `Option<&str>` / `Option<String>`
  - numbers: ids and counts `i64`; fractional values `f64`
  - booleans: `bool`
  - objects, arrays, `any`: `&Value` in, `Value` out
  - `string[]`: `&[String]` in, `Vec<String>` out
  - a string union (`"pending" | "queue" | "send"`): `&'static str`
  - `Date`: `chrono::DateTime<chrono::Local>`
  - "X or an error string" returns (`scheduleIn`): `Result<Value, String>`
  - TS optional/default parameters become ordinary parameters; callers pass the default.
- **Database**: `db::all(sql, params![..]) -> Vec<Value>`, `db::get(..) -> Option<Value>`,
  `db::run(..) -> RunResult { changes, last_insert_rowid }`, `db::exec(sql)`, `db::transaction(|| { .. })`. Rows are
  JSON objects keyed by column, exactly as bun:sqlite returned them (INTEGER → number, TEXT → string, NULL → null).
  Never hold `db::conn()` across an `.await`.
- **Errors**: functions that can fail return `anyhow::Result`. `throw httpError(404, "…")` becomes
  `fail!(404, "…")` (or `Err(util::http_error(404, msg))`); the HTTP layer answers `{ error }` with that status, and
  any other error with 500 and its message, as before.
- **Async**: `async function` → `async fn`. A promise nobody awaits → `util::spawn(async move { .. })`.
  `setTimeout` → `util::after(ms, async move { .. })` or `tokio::time::sleep`. `setInterval` → a spawned loop.
  Module-level `Map`/`Set`/`let` state → a `static` behind `std::sync::Mutex` (`LazyLock` or `OnceLock`); never hold
  such a lock across an `.await`.
- **Processes**: `sh(cmd, opts)` → `util::sh(&[..], ShOpts { .. }).await` (same `ok/code/out/err`).
  `Bun.spawnSync` → `util::sh_sync`. `Bun.which` → `util::which`. Long-lived processes: `tokio::process::Command`.
- **Paths and files**: `util::join/basename/dirname/extname/exists/realpath`, `std::fs`. `HOME` → `util::home()`,
  `PORT` → `util::port()`, `HOST` → `util::HOST`, `now()` → `util::now()` (JS `toISOString` format; timestamps are
  compared as strings, keep the format).
- **Live updates** (`server.rs`): `emit(json!({..}))`, `note_todo(id, Part::Full | Part::Summary | Part::Event(id), git)`,
  `changed(Option<i64>)`, `set_todo(id, json!({ field: value }), touch)`, `add_message(..)`, `add_event(..)`,
  `set_event(..)` — same rules as in `server.ts`.
- **No new behaviour, no dropped behaviour.** If something cannot be ported faithfully, leave a `// PORT:` comment
  saying what differs and why.
- Code style: comments say why, not what; no comment on obvious code. `cargo clippy` clean.

## Done means (for a change)

`cargo build`, `cargo clippy` and `cargo test` pass. Tests that touch the database take `db::test_db()`.
