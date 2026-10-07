import { expect, test } from "bun:test";
import {
  contextFile, cutToTwoLines, deliveryAction, deliveryPrompt, ghOutcome, handOver, isInside, lineCount, migrateDb, parseGitStatus, parseMarks, parseOscTitle, versionAtLeast, parseGithubRemote, parseRefCounts, ctxStep, planCount, toolMeta,
  prFromNode, safeFileName, slugify, statusAfterRun, tokenOk, toolArg, toolLine, uniqueSlug, withAttachments,
} from "./lib";

test("slugify and uniqueSlug", () => {
  expect(slugify("Billing API!")).toBe("billing-api");
  expect(slugify("  Café / Web  ")).toBe("cafe-web");
  expect(slugify("!!!")).toBe("project");
  expect(uniqueSlug("api", new Set())).toBe("api");
  expect(uniqueSlug("api", new Set(["api", "api-2"]))).toBe("api-3");
});

test("cutToTwoLines", () => {
  expect(cutToTwoLines("  one\n\ntwo  ")).toBe("one\ntwo");
  expect(cutToTwoLines("one\ntwo\nthree")).toBe("one\ntwo…");
  const long = cutToTwoLines("x".repeat(500));
  expect(long.length).toBe(280);
  expect(long.endsWith("…")).toBe(true);
  expect(lineCount("a\n\nb\n")).toBe(2);
});

test("deliveryPrompt", () => {
  expect(deliveryPrompt(null, ["fix it", " also tests "])).toBe("fix it\n\nalso tests");
  expect(deliveryPrompt("/w/feat", ["go"])).toBe("Your worktree: /w/feat. Work there.\n\ngo");
  expect(deliveryPrompt(null, [])).toBe("continue");
  expect(deliveryPrompt("/w/feat", ["", " "])).toBe("Your worktree: /w/feat. Work there.\n\ncontinue");
});

test("withAttachments and safeFileName", () => {
  expect(withAttachments("fix it", [])).toBe("fix it");
  expect(withAttachments(" cut off ", ["/u/1-1-a.png", "/u/1-2-b.pdf"])).toBe("cut off\n\nAttached files (open with Read):\n/u/1-1-a.png\n/u/1-2-b.pdf");
  expect(withAttachments("", ["/u/1-1-a.png"])).toBe("Attached files (open with Read):\n/u/1-1-a.png");
  expect(safeFileName("../My Shot (1).png")).toBe("My-Shot-1-.png");
  expect(safeFileName("..")).toBe("file");
  expect(safeFileName(".env")).toBe("env");
  expect(safeFileName("a/b\\c.txt")).toBe("b-c.txt");
  expect(safeFileName("x".repeat(300) + ".png")).toBe("x".repeat(116) + ".png");
});

test("deliveryAction and statusAfterRun", () => {
  expect(deliveryAction("not_started")).toBe("pending");
  expect(deliveryAction("running")).toBe("queue");
  expect(deliveryAction("idle")).toBe("send");
  expect(deliveryAction("waiting")).toBe("send");
  expect(statusAfterRun(1)).toBe("waiting");
  expect(statusAfterRun(false)).toBe("idle");
});

test("isInside", () => {
  expect(isInside("/h/data/p/1/plan.md", "/h/data/p/1")).toBe(true);
  expect(isInside("/h/data/p/1/sub/x.md", "/h/data/p/1")).toBe(true);
  expect(isInside("/h/data/p/1", "/h/data/p/1")).toBe(false);
  expect(isInside("/h/data/p/12/x.md", "/h/data/p/1")).toBe(false);
  expect(isInside("/h/data/p/1/../2/x.md", "/h/data/p/1")).toBe(false);
});

test("toolArg and toolLine", () => {
  expect(toolArg({ command: "git push --force origin main", description: "x" })).toBe("git push --force origin main");
  expect(toolLine("Bash", { command: "git status" })).toBe("Bash git status");
  expect(toolLine("mcp__app__set_title", { title: "x" })).toBe("mcp__app__set_title");
});

test("parseGitStatus and parseRefCounts", () => {
  // AI-999-finance-luna: two modified files and one untracked file
  const status = "# branch.oid e3e\n# branch.head feature/AI-999\n# branch.upstream origin/feature/AI-999\n# branch.ab +0 -0\n"
    + "1 .M N... 100644 100644 100644 e3 e3 a.py\n1 .M N... 100644 100644 100644 4b 4b b.py\n? docs/plan.md";
  expect(parseGitStatus(status)).toEqual({ branch: "feature/AI-999", changed: 2, untracked: 1 });
  expect(parseGitStatus("# branch.head (detached)").branch).toBe(null);
  // AI-1026: tracks origin/main (branch.ab +7 -5) but was never pushed: no origin/<branch> line
  expect(parseRefCounts("refs/remotes/origin/HEAD origin/main 5 7\n", "bugfix/AI-1026"))
    .toEqual({ base: "main", behind: 5, commits: 7, push: "none", unpushed: 0 });
  // renovate: pushed, then one more commit
  expect(parseRefCounts("refs/remotes/origin/HEAD origin/main 2 3\nrefs/remotes/origin/renovate  0 1\n", "renovate"))
    .toEqual({ base: "main", behind: 2, commits: 3, push: "unpushed", unpushed: 1 });
  expect(parseRefCounts("refs/remotes/origin/x  0 0", "x")).toEqual({ base: null, behind: null, commits: null, push: "pushed", unpushed: 0 });
});

test("parseGithubRemote", () => {
  expect(parseGithubRemote("git@github.com:Hostaway/copilot-service.git")).toEqual({ owner: "Hostaway", name: "copilot-service" });
  expect(parseGithubRemote("https://github.com/himmelreich-it/switchboard.git\n")).toEqual({ owner: "himmelreich-it", name: "switchboard" });
  expect(parseGithubRemote("https://gitlab.com/a/b.git")).toBe(null);
});

test("prFromNode and ghOutcome", () => {
  // PR #522 as GitHub returned it: open, three failing checks, behind main
  const pr = prFromNode({
    number: 522, url: "https://github.com/Hostaway/copilot-service/pull/522", state: "OPEN", isDraft: false, mergeable: "MERGEABLE",
    mergeStateStatus: "BEHIND", reviewDecision: "REVIEW_REQUIRED", baseRefName: "main", reviewThreads: { nodes: [{ isResolved: true }] },
    commits: { nodes: [{ commit: { statusCheckRollup: { state: "FAILURE", contexts: { nodes: [
      { __typename: "CheckRun", name: "lint", status: "COMPLETED", conclusion: "FAILURE" },
      { __typename: "CheckRun", name: "deploy", status: "COMPLETED", conclusion: "SKIPPED" },
      { __typename: "CheckRun", name: "e2e", status: "IN_PROGRESS", conclusion: null },
      { __typename: "StatusContext", context: "ci/build", state: "ERROR" },
      { __typename: "StatusContext", context: "codecov", state: "SUCCESS" },
    ] } } } }] },
  });
  expect(pr).toMatchObject({ state: "open", conflicts: false, mergeable: true, behind: true, base: "main", review: "required", threads: 0 });
  expect(pr.checks).toEqual({ state: "failing", failing: ["lint", "ci/build"], running: 1, passed: 1, skipped: 1 });
  expect(prFromNode({ number: 1, state: "MERGED", mergeable: "UNKNOWN" })).toMatchObject({ state: "merged", conflicts: false, mergeable: false });

  const notFound = '{"data":{"repository":null},"errors":[{"type":"NOT_FOUND","path":["repository"],"message":"Could not resolve"}]}';
  expect(ghOutcome(1, notFound, "gh: Could not resolve")).toEqual({ error: "no access", detail: "" });
  expect(ghOutcome(1, '{"message":"Bad credentials"}', "gh: Bad credentials (HTTP 401)")).toEqual({ error: "no access", detail: "" });
  const partial = '{"data":{"repository":{"b0":{"nodes":[]},"n0":null}},"errors":[{"type":"NOT_FOUND","path":["repository","n0"]}]}';
  expect(ghOutcome(1, partial, "")).toEqual({ repo: { b0: { nodes: [] }, n0: null } });
  expect(ghOutcome(1, "", 'Post "https://api.github.com/graphql": dial tcp 127.0.0.1:9: connect: connection refused')).toMatchObject({ error: "no network" });
  expect(ghOutcome(1, "", "gh: Something went wrong")).toEqual({ error: "unclear", detail: "exit 1 · gh: Something went wrong" });
});

test("tokenOk", () => {
  const tok = "ab".repeat(32);
  expect(tokenOk(tok, tok, null)).toBe(true);
  expect(tokenOk(tok, null, tok)).toBe(true);
  expect(tokenOk(tok, null, null)).toBe(false);
  expect(tokenOk(tok, "ab", null)).toBe(false);
  expect(tokenOk(tok, "cd".repeat(32), tok)).toBe(false); // the header wins over the query
});

test("handOver leaves out lines without a value", () => {
  const full = handOver({
    no: 3, title: "Implement MCPs (AI-1234)", worktree: "/w/ai-1234", pr: 512,
    last: { no: 2, body: "Server done,\nclient stubs left." },
    files: [{ path: "/d/plan.md", session_no: 1 }, { path: "/d/notes.md", session_no: null }],
  });
  expect(full).toBe([
    "This TODO continues earlier work (session 3). Title: Implement MCPs (AI-1234). Keep this title.",
    "Your worktree: /w/ai-1234. Work there.",
    "Pull request: #512.",
    "Last message of session 2: Server done, client stubs left.",
    "Files from earlier sessions (open with Read what you need):",
    "/d/plan.md (session 1)",
    "/d/notes.md (added outside a session)",
  ].join("\n"));
  expect(handOver({ no: 2, title: "T", worktree: null, pr: null, last: null, files: [] }))
    .toBe("This TODO continues earlier work (session 2). Title: T. Keep this title.");
  // the header names the worktree, so the usual worktree line is not repeated
  expect(deliveryPrompt("/w", ["go"], "HEADER")).toBe("HEADER\n\ngo");
});

test("ctxStep reads the context from usage_update", () => {
  const ups = [
    { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "hi" } },
    { sessionUpdate: "usage_update", used: 24532, size: 1000000, _meta: { "_claude/model": "claude-opus-5-5" } },
    { sessionUpdate: "usage_update", used: 25010, size: 1000000 },
  ];
  const c = ups.reduce<any>((c, u) => ctxStep(c, u), null);
  expect(c).toEqual({ used: 25010, window: 1000000, turns: 0, model: "claude-opus-5-5" });
  expect(ctxStep(null, { sessionUpdate: "plan" })).toBe(null);
  expect(ctxStep(c, { sessionUpdate: "usage_update", used: 900000, size: 1000000, _meta: { "_claude/origin": { kind: "task-notification" } } })).toBe(c);
});

test("migrateDb makes earlier TODOs session 1 and runs twice without harm", () => {
  const { Database } = require("bun:sqlite");
  const db = new Database(":memory:");
  // a database from before sessions
  db.exec(`CREATE TABLE todos (id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL, title TEXT NOT NULL, status TEXT NOT NULL, attention INTEGER NOT NULL DEFAULT 0,
    session_id TEXT, worktree_path TEXT, last_tool TEXT, last_read_at TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, removed_at TEXT, removed_by TEXT);
    CREATE TABLE messages (id INTEGER PRIMARY KEY, todo_id INTEGER, author TEXT NOT NULL, body TEXT NOT NULL, state TEXT NOT NULL, file_id INTEGER, committed_at TEXT NOT NULL, delivered_at TEXT);
    CREATE TABLE events (id INTEGER PRIMARY KEY, todo_id INTEGER, kind TEXT NOT NULL, actor TEXT NOT NULL, detail TEXT, at TEXT NOT NULL);
    CREATE TABLE files (id INTEGER PRIMARY KEY, todo_id INTEGER NOT NULL, path TEXT NOT NULL UNIQUE);
    INSERT INTO todos VALUES (1, 1, 'started', 'idle', 0, 'sid-1', NULL, NULL, NULL, 't0', 't9', NULL, NULL);
    INSERT INTO todos VALUES (2, 1, 'idea', 'not_started', 0, NULL, NULL, NULL, NULL, 't0', 't0', NULL, NULL);
    INSERT INTO messages (todo_id, author, body, state, committed_at) VALUES (1, 'user', 'go', 'delivered', 't1'), (1, 'user', 'later', 'queued', 't5'), (2, 'user', 'idea', 'pending', 't0');
    INSERT INTO events (todo_id, kind, actor, at) VALUES (1, 'created', 'user', 't0'), (1, 'launch', 'app', 't2'), (1, 'run ended', 'app', 't3');
    INSERT INTO files (todo_id, path) VALUES (1, '/d/plan.md');`);
  migrateDb(db);
  migrateDb(db);
  expect(db.query("SELECT todo_id, no, session_id, started_at FROM sessions").all()).toEqual([{ todo_id: 1, no: 1, session_id: "sid-1", started_at: "t2" }]);
  expect(db.query("SELECT body, session_no FROM messages ORDER BY id").all().map((m: any) => m.session_no)).toEqual([1, null, null]);
  expect(db.query("SELECT kind, session_no FROM events ORDER BY id").all().map((e: any) => e.session_no)).toEqual([null, 1, 1]);
  expect((db.query("SELECT session_no FROM files").get() as any).session_no).toBe(1);
  expect(db.query("SELECT 1 FROM pragma_table_info('todos') WHERE name = 'ctx'").get()).toBeTruthy();
});

test("parseMarks", () => {
  const ns = ["M", "src/a.ts", "A", "src/new.ts", "D", "old.md", "R087", "x/from.ts", "x/to.ts", ""].join("\0");
  expect(parseMarks(ns, ["notes.txt", "tmp/u.log", ""].join("\0"))).toEqual({
    "src/a.ts": "M", "src/new.ts": "A", "old.md": "D", "x/from.ts": "D", "x/to.ts": "A", "notes.txt": "U", "tmp/u.log": "U",
  });
  expect(parseMarks("", "")).toEqual({});
});

test("terminal helpers", () => {
  expect(parseOscTitle("abc\x1b]0;zsh — feat-x\x07more\x1b]2;Claude Code\x1b\\")).toBe("Claude Code");
  expect(parseOscTitle("plain output")).toBe(null);
  expect(versionAtLeast("1.3.14", "1.3.5")).toBe(true);
  expect(versionAtLeast("1.3.4", "1.3.5")).toBe(false);
  expect(versionAtLeast("1.4.0", "1.3.5")).toBe(true);
  const ctx = contextFile({
    title: "T", project: "p", status: "idle", first: "do it", worktree: "/w", branch: "b", git: "2 commits · pushed", pr: "#5 open",
    sessions: [{ no: 1, session_id: "s1", started_at: "a", ended_at: "b" }, { no: 2, session_id: "s2", started_at: "c", ended_at: null }],
    messages: [{ author: "user", body: "do it", at: "a", session_no: 1 }, { author: "agent", body: "done", at: "b", session_no: 2 }, { author: "user", body: "queued", at: "d", session_no: null }],
    files: ["/d/plan.md"],
  });
  expect(ctx).toContain("- Worktree: /w");
  expect(ctx).toContain("### Session 2\n\n**agent** (b): done\n\n**user** (d): queued");
  expect(ctx).toContain("Check with the user before you commit");
  expect(contextFile({ title: "T", project: "p", status: "not_started", first: "x", worktree: null, branch: null, git: null, pr: null, sessions: [], messages: [], files: [] }))
    .toContain("not started (in the backlog)");
});

test("toolMeta merges a tool call's updates; planCount", () => {
  const ups = [
    { sessionUpdate: "tool_call", toolCallId: "t1", title: "Write", kind: "edit", status: "pending", rawInput: {}, _meta: { claudeCode: { toolName: "Write" } } },
    { sessionUpdate: "tool_call_update", toolCallId: "t1", title: "Write b.txt", rawInput: { file_path: "/r/b.txt", content: "x" },
      content: [{ type: "diff", path: "/r/b.txt", oldText: null, newText: "x" }], locations: [{ path: "/r/b.txt" }] },
    { sessionUpdate: "tool_call_update", toolCallId: "t1", status: "completed", content: [{ type: "content", content: { type: "text", text: "done" } }] },
  ];
  const m = ups.reduce<any>((m, u) => toolMeta(m, u), null);
  expect(m).toMatchObject({ name: "Write", title: "Write b.txt", kind: "edit", status: "completed", input: { file_path: "/r/b.txt" }, output: "done",
    locations: [{ path: "/r/b.txt" }], diffs: [{ path: "/r/b.txt", oldText: null, newText: "x" }] });
  expect(toolMeta(null, { toolCallId: "t2", rawOutput: "x".repeat(9000), status: "completed" }).output.length).toBeLessThan(8100);
  expect(planCount(null)).toBe(null);
  expect(planCount([{ content: "a", status: "completed" }, { content: "b", status: "pending" }])).toBe("1/2");
});
