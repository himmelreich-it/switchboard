// Dumps what apptools.ts (the real module, with the real MCP SDK and zod) answers, as golden files for the Rust port.
import { grant, handleMcp } from "./apptools";

const out = process.argv[2];
const grants: Record<string, string> = {
  worker: grant({ kind: "worker", todoId: 1, dir: "/tmp/sb-data/1", onPost: () => {} }, (s) => s).secret,
  worker_terminal: grant({ kind: "worker", todoId: 1, dir: "/tmp/sb-data/1", onPost: () => {}, via: "Claude 2" }, (s) => s).secret,
  worker_codex: grant({ kind: "worker", todoId: 2, dir: "/tmp/sb-data/2", onPost: () => {} }, (s) => s).secret,
  operator: grant({ kind: "operator" }, (s) => s).secret,
  router: grant({ kind: "router", todoId: 1 }, (s) => s).secret,
};

type Case = { name: string; grant?: string; method?: string; headers?: Record<string, string | null>; body?: unknown; raw?: string };
const rpc = (method: string, params?: unknown, id: number | string = 1) => ({ jsonrpc: "2.0", id, method, ...(params === undefined ? {} : { params }) });
const call = (name: string, args?: unknown) => rpc("tools/call", args === undefined ? { name } : { name, arguments: args });
const init = (v: string) => rpc("initialize", { protocolVersion: v, capabilities: {}, clientInfo: { name: "test", version: "0" } });

const cases: Case[] = [
  { name: "initialize, supported version", body: init("2025-06-18") },
  { name: "initialize, unknown version", body: init("1999-01-01") },
  { name: "initialize, operator", grant: "operator", body: init("2025-11-25") },
  { name: "initialize, invalid params", body: rpc("initialize", { protocolVersion: 1 }) },
  { name: "initialized notification", body: { jsonrpc: "2.0", method: "notifications/initialized" } },
  { name: "unknown notification", body: { jsonrpc: "2.0", method: "notifications/whatever", params: { a: 1 } } },
  { name: "ping", body: rpc("ping") },
  { name: "ping, string id", body: rpc("ping", {}, "abc") },
  { name: "unknown method", body: rpc("resources/list", {}, 7) },
  { name: "tools/list, router", grant: "router", body: rpc("tools/list") },
  { name: "batch", body: [rpc("ping", undefined, 1), { jsonrpc: "2.0", method: "notifications/initialized" }, rpc("nope", undefined, 2)] },
  { name: "batch of one", body: [rpc("ping", undefined, 5)] },
  { name: "batch of notifications", body: [{ jsonrpc: "2.0", method: "notifications/initialized" }] },
  { name: "empty batch", body: [] },
  { name: "response message", body: { jsonrpc: "2.0", id: 9, result: {} } },
  { name: "batch too large", body: Array.from({ length: 101 }, (_, i) => rpc("ping", undefined, i)) },
  { name: "bad secret", headers: { "x-switchboard-session": "nope" }, body: rpc("ping") },
  { name: "no secret", headers: { "x-switchboard-session": null }, body: rpc("ping") },
  { name: "GET", method: "GET", headers: { accept: "text/event-stream", "content-type": null } },
  { name: "DELETE", method: "DELETE", headers: { "content-type": null } },
  { name: "PUT", method: "PUT", body: rpc("ping") },
  { name: "bad secret, GET", method: "GET", headers: { "x-switchboard-session": "nope" } },
  { name: "accept json only", headers: { accept: "application/json" }, body: rpc("ping") },
  { name: "no accept", headers: { accept: null }, body: rpc("ping") },
  { name: "text content type", headers: { "content-type": "text/plain" }, body: rpc("ping") },
  { name: "json content type with charset", headers: { "content-type": "Application/JSON; charset=utf-8" }, body: rpc("ping") },
  { name: "invalid json", raw: "{nope" },
  { name: "empty body", raw: "" },
  { name: "not json-rpc", body: { foo: 1 } },
  { name: "extra key", body: { ...rpc("ping"), extra: 1 } },
  { name: "wrong jsonrpc version", body: { jsonrpc: "1.0", id: 1, method: "ping" } },
  { name: "float id", body: { jsonrpc: "2.0", id: 1.5, method: "ping" } },
  { name: "unsupported protocol header", headers: { "mcp-protocol-version": "1999-01-01" }, body: rpc("ping") },
  { name: "supported protocol header", headers: { "mcp-protocol-version": "2025-06-18" }, body: rpc("ping") },
  { name: "tools/call, unknown tool", body: call("nope", {}) },
  { name: "tools/call, operator tool from a worker", body: call("list_todos", {}) },
  { name: "tools/call, no params", body: rpc("tools/call") },
  { name: "tools/call, no name", body: rpc("tools/call", { arguments: {} }) },
  { name: "tools/call, arguments a string", body: rpc("tools/call", { name: "set_title", arguments: "x" }) },
  { name: "tools/call, arguments an array", body: rpc("tools/call", { name: "set_title", arguments: [1] }) },
  { name: "post_message, no arguments", body: call("post_message") },
  { name: "post_message, wrong types", body: call("post_message", { body: 1, file_name: null }) },
  { name: "post_message, empty body", body: call("post_message", { body: "  " }) },
  { name: "post_message, three lines", body: call("post_message", { body: "a\nb\nc" }) },
  { name: "set_title, empty", body: call("set_title", { title: " ", extra: 1 }) },
  { name: "set_attention, string", body: call("set_attention", { raised: "yes" }) },
  { name: "attach_file, relative path", body: call("attach_file", { path: "a.md" }) },
  { name: "attach_file, missing file", body: call("attach_file", { path: "/nonexistent/sb/a.md" }) },
  { name: "set_worktree, relative path", body: call("set_worktree", { path: "wt" }) },
  { name: "set_pr, zero", body: call("set_pr", { number: 0 }) },
  { name: "set_pr, fraction", body: call("set_pr", { number: 1.5 }) },
  { name: "set_pr, negative fraction", body: call("set_pr", { number: -1.5 }) },
  { name: "set_pr, string", body: call("set_pr", { number: "3" }) },
  { name: "set_pr, too big", body: call("set_pr", { number: 9007199254740993 }) },
  { name: "set_pr, too small", raw: '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"set_pr","arguments":{"number":-9007199254740993}}}' },
  { name: "set_pr, exponent", raw: '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"set_pr","arguments":{"number":1e21}}}' },
  { name: "reply_comment, float-valued int", raw: '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"reply_comment","arguments":{"id":2.0,"reply":" "}}}' },
  { name: "set_effort, bad level", body: call("set_effort", { level: "max" }) },
  { name: "set_effort, from a terminal", grant: "worker_terminal", body: call("set_effort", { level: "low", reason: "x" }) },
  { name: "set_effort, codex", grant: "worker_codex", body: call("set_effort", { level: "low", reason: "x" }) },
  { name: "suggest_follow_ups, bad items", body: call("suggest_follow_ups", { items: [1, "a", null] }) },
  { name: "suggest_follow_ups, object", body: call("suggest_follow_ups", { items: {} }) },
  { name: "suggest_follow_ups, none", body: call("suggest_follow_ups", { items: [] }) },
  { name: "reply_comment, empty reply", body: call("reply_comment", { id: 3, reply: "" }) },
  { name: "reply_comment, missing id", body: call("reply_comment", { reply: "ok" }) },
  { name: "set_schedule, no schedule key", body: call("set_schedule", { todo_id: 3 }) },
  { name: "remove_todo, bool id", body: call("remove_todo", { todo_id: true }) },
  { name: "create_todo, empty", body: call("create_todo", {}) },
  { name: "create_todo, wrong optionals", grant: "operator", body: call("create_todo", { instruction: "x", title: null, start: "yes", topic: 3 }) },
  { name: "create_todo, empty instruction", grant: "operator", body: call("create_todo", { instruction: " " }) },
  { name: "list_todos, bad flag", grant: "operator", body: call("list_todos", { include_removed: 1 }) },
  { name: "send_message, missing", grant: "operator", body: call("send_message", {}) },
  { name: "read_file, wrong types", grant: "operator", body: call("read_file", { todo_id: "1", name: 2 }) },
  { name: "route_todo, bad effort", grant: "router", body: call("route_todo", { topic: "Alpha", reason: "r", effort: "max" }) },
  { name: "ask_topic, options a string", grant: "router", body: call("ask_topic", { question: "q", options: "Alpha" }) },
  { name: "post_message from the router", grant: "router", body: call("post_message", { body: "x" }) },
  { name: "tools/call with a task", body: rpc("tools/call", { name: "set_title", arguments: { title: " " }, task: {} }) },
  { name: "tools/call with an invalid task", body: rpc("tools/call", { name: "set_title", arguments: { title: " " }, task: { ttl: "x" } }) },
  { name: "ping with a task", body: rpc("ping", { task: { ttl: 5 } }) },
  { name: "unknown method with a task", body: rpc("nope", { task: {} }) },
  { name: "tools/list, numeric cursor", body: rpc("tools/list", { cursor: 5 }) },
  { name: "tools/list, string cursor", grant: "router", body: rpc("tools/list", { cursor: "x", _meta: { progressToken: "t" } }) },
  { name: "initialize, no params", body: rpc("initialize") },
  { name: "initialize, clientInfo without version", body: rpc("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: 1, title: 2, icons: 3 } }) },
  { name: "initialize, capabilities a number", body: rpc("initialize", { protocolVersion: "2025-06-18", capabilities: 5, clientInfo: { name: "a", version: "1" } }) },
  { name: "initialize, protocolVersion number only", body: rpc("initialize", { protocolVersion: 2025, capabilities: {}, clientInfo: { name: "a", version: "1" } }) },
  { name: "params null", body: { jsonrpc: "2.0", id: 1, method: "ping", params: null } },
  { name: "params array", body: { jsonrpc: "2.0", id: 1, method: "ping", params: [] } },
  { name: "_meta with a float progress token", body: rpc("ping", { _meta: { progressToken: 1.5 } }) },
  { name: "_meta a string", body: rpc("ping", { _meta: "x" }) },
  { name: "id null", body: { jsonrpc: "2.0", id: null, method: "ping" } },
  { name: "integer-valued float id", raw: '{"jsonrpc":"2.0","id":3.0,"method":"ping"}' },
  { name: "negative id", body: rpc("ping", undefined, -4) },
  { name: "method a number", body: { jsonrpc: "2.0", id: 1, method: 5 } },
  { name: "cancelled notification", body: { jsonrpc: "2.0", method: "notifications/cancelled", params: { requestId: 1 } } },
  { name: "error response message", body: { jsonrpc: "2.0", id: 1, error: { code: -1, message: "x" } } },
  { name: "tools/call, arguments null", body: rpc("tools/call", { name: "set_title", arguments: null }) },
  { name: "whitespace around json", raw: '  {"jsonrpc":"2.0","id":1,"method":"ping"}\n' },
  { name: "json content type with bad params", headers: { "content-type": "application/json; a=b,c" }, body: rpc("ping") },
  { name: "json content type with trailing semicolon", headers: { "content-type": "application/json;" }, body: rpc("ping") },
  { name: "json content type with quoted param", headers: { "content-type": "application/json; a=\"b,c\"" }, body: rpc("ping") },
  { name: "content type with space", headers: { "content-type": " application/json " }, body: rpc("ping") },
  { name: "accept wildcard", headers: { accept: "*/*" }, body: rpc("ping") },
];

const results = [];
for (const c of cases) {
  const secret = grants[c.grant ?? "worker"];
  const h: Record<string, string> = { "x-switchboard-session": secret, accept: "application/json, text/event-stream", "content-type": "application/json" };
  for (const [k, v] of Object.entries(c.headers ?? {})) {
    if (v === null) delete h[k];
    else h[k] = v;
  }
  const method = c.method ?? "POST";
  const body = c.raw ?? (c.body === undefined ? undefined : JSON.stringify(c.body));
  const res = await handleMcp(new Request("http://127.0.0.1:4477/mcp/app", { method, headers: h, body: method === "GET" || method === "HEAD" ? undefined : body }));
  const text = await res.text();
  const shown = { ...h };
  if (shown["x-switchboard-session"] === secret) shown["x-switchboard-session"] = "$secret";
  results.push({
    name: c.name, grant: c.grant ?? "worker", method, headers: shown, body: body ?? null,
    status: res.status, content_type: res.headers.get("content-type"), allow: res.headers.get("allow"), response: text,
  });
}
await Bun.write(`${out}/apptools-protocol.json`, JSON.stringify(results, null, 1) + "\n");

const lists: Record<string, unknown> = {};
for (const g of Object.keys(grants)) {
  const res = await handleMcp(new Request("http://127.0.0.1:4477/mcp/app", {
    method: "POST", headers: { "x-switchboard-session": grants[g], accept: "application/json, text/event-stream", "content-type": "application/json" },
    body: JSON.stringify(rpc("tools/list")),
  }));
  lists[g] = JSON.parse(await res.text()).result;
}
await Bun.write(`${out}/apptools-tools-list.json`, JSON.stringify(lists, null, 1) + "\n");
console.log(`${results.length} cases, ${Object.keys(lists).length} tool lists`);
