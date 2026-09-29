import { query, type Options } from "@anthropic-ai/claude-agent-sdk";
import { Database } from "bun:sqlite";
import { existsSync, mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import {
  buildReview, diffLines, extractCommitMessage, extractJson, findWorktree, mergeFindings,
  parsePrUrl, readOnlyDenyReason, validFindings, type Finding,
} from "./lib";

// No login on this server: it must only ever listen on 127.0.0.1.
const HOST = "127.0.0.1";
const PORT = Number(process.env.PORT ?? 4477);

// ---------- storage ----------

const dir = join(homedir(), ".auto-coder");
mkdirSync(dir, { recursive: true });
const db = new Database(join(dir, "db.sqlite"));
db.exec(`
CREATE TABLE IF NOT EXISTS projects (id INTEGER PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE IF NOT EXISTS reviews (
  id INTEGER PRIMARY KEY, pr_url TEXT NOT NULL, project_id INTEGER, pr_number INTEGER, is_mine INTEGER,
  worktree_path TEXT, session_id TEXT, status TEXT, summary TEXT, created_at TEXT DEFAULT CURRENT_TIMESTAMP);
CREATE TABLE IF NOT EXISTS findings (
  id INTEGER NOT NULL, review_id INTEGER NOT NULL, file TEXT, line INTEGER, severity TEXT, body TEXT,
  suggested_fix TEXT, selected INTEGER DEFAULT 0, posted INTEGER DEFAULT 0, edited INTEGER DEFAULT 0,
  PRIMARY KEY (review_id, id));
CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY, review_id INTEGER, role TEXT, content TEXT, created_at TEXT DEFAULT CURRENT_TIMESTAMP);
UPDATE reviews SET status = 'error' WHERE status = 'running';
`);

const DEFAULT_PROMPT = `Review this pull request. Read the PR description and its existing review comments first
(gh pr view, gh api repos/{owner}/{repo}/pulls/{n}/comments) and do not repeat points already raised.
Look at the diff against the base branch and report bugs, security problems and risky changes.`;

const getSetting = (key: string, fallback: string) =>
  (db.query("SELECT value FROM settings WHERE key = ?").get(key) as any)?.value ?? fallback;
const model = () => getSetting("model", "claude-opus-5-5");
const review = (id: number) => db.query("SELECT * FROM reviews WHERE id = ?").get(id) as any;
const findings = (id: number) => db.query("SELECT * FROM findings WHERE review_id = ? ORDER BY id").all(id) as Finding[];

function saveFindings(reviewId: number, list: Finding[]) {
  db.transaction(() => {
    db.run("DELETE FROM findings WHERE review_id = ?", [reviewId]);
    for (const f of list)
      db.run(
        "INSERT INTO findings (id, review_id, file, line, severity, body, suggested_fix, selected, posted, edited) VALUES (?,?,?,?,?,?,?,?,?,?)",
        [f.id, reviewId, f.file, f.line, f.severity, f.body, f.suggested_fix ?? null, f.selected ?? 0, f.posted ?? 0, f.edited ?? 0],
      );
  })();
}

// ---------- live updates (one SSE stream, client filters by review_id) ----------

const clients = new Set<ReadableStreamDefaultController>();
const enc = new TextEncoder();
function emit(ev: object) {
  const chunk = enc.encode(`data: ${JSON.stringify(ev)}\n\n`);
  for (const c of clients) try { c.enqueue(chunk); } catch { clients.delete(c); }
}
function log(reviewId: number, role: string, content: string) {
  db.run("INSERT INTO messages (review_id, role, content) VALUES (?,?,?)", [reviewId, role, content]);
  emit({ type: "message", review_id: reviewId, role, content });
}
function setReview(id: number, fields: Record<string, any>) {
  const keys = Object.keys(fields);
  db.run(`UPDATE reviews SET ${keys.map((k) => `${k} = ?`).join(", ")} WHERE id = ?`, [...keys.map((k) => fields[k]), id]);
  emit({ type: "review", review_id: id });
}

// ---------- shell ----------

async function sh(cmd: string[], opts: { cwd?: string; stdin?: string } = {}) {
  const p = Bun.spawn(cmd, { cwd: opts.cwd, stdin: opts.stdin ? new Blob([opts.stdin]) : "ignore", stdout: "pipe", stderr: "pipe" });
  const [out, err, code] = await Promise.all([new Response(p.stdout).text(), new Response(p.stderr).text(), p.exited]);
  return { ok: code === 0, out: out.trim(), err: err.trim() || out.trim() };
}
async function must(cmd: string[], opts?: { cwd?: string; stdin?: string }) {
  const r = await sh(cmd, opts);
  if (!r.ok) throw new Error(`${cmd.join(" ")}: ${r.err}`);
  return r.out;
}
const ghLogin = async () => (await sh(["gh", "api", "user", "--jq", ".login"])).out || null;

// ---------- Claude runs ----------

function toolLine(name: string, input: any) {
  const arg = input?.command ?? input?.file_path ?? input?.pattern ?? input?.skill ?? input?.path ?? "";
  return `${name} ${String(arg)}`.slice(0, 300);
}

// Runs one query, logs text and tool calls to the chat, returns the final result.
async function runClaude(reviewId: number, prompt: string, options: Options) {
  for await (const m of query({ prompt, options })) {
    if (m.type === "system" && m.subtype === "init" && !options.resume) setReview(reviewId, { session_id: m.session_id });
    if (m.type === "assistant") {
      for (const b of m.message.content as any[]) {
        if (b.type === "tool_use") log(reviewId, "tool", toolLine(b.name, b.input));
        if (b.type === "text" && b.text.trim()) {
          // The findings JSON shows as cards, so the chat gets a one-line stand-in instead of the raw JSON.
          const j = extractJson(b.text).value;
          const stub = j && `[findings JSON: ${j.findings.length} findings]`;
          let shown = j ? b.text.replace(/```(?:json)?\s*\n[\s\S]*?```/g, (f: string) => (extractJson(f).value ? stub : f)) : b.text;
          if (j && shown === b.text) shown = stub;
          log(reviewId, "assistant", shown);
        }
      }
    }
    if (m.type === "result") {
      if (m.subtype !== "success") throw new Error(`Claude run ended with ${m.subtype}: ${(m as any).errors?.join("; ") ?? ""}`);
      return { text: m.result, structured: m.structured_output as any };
    }
  }
  throw new Error("Claude run ended without a result");
}

// Review and chat sessions are read-only. The PreToolUse hook enforces it, because allow rules
// (allowedTools, project settings) skip canUseTool, so canUseTool would never see `gh api -X POST`.
function readOnlyOptions(cwd: string, resume?: string): Options {
  return {
    cwd, model: model(), resume, settingSources: ["project"],
    allowedTools: ["Read", "Grep", "Glob", "Bash(git diff:*)", "Bash(git log:*)", "Bash(gh pr view:*)", "Bash(gh pr diff:*)", "Bash(gh api:*)"],
    hooks: {
      PreToolUse: [{
        hooks: [async (input: any) => {
          const reason = readOnlyDenyReason(input.tool_name, input.tool_input);
          return reason ? { hookSpecificOutput: { hookEventName: "PreToolUse" as const, permissionDecision: "deny" as const, permissionDecisionReason: reason } } : {};
        }],
      }],
    },
  };
}

const busy = new Set<number>();
async function withBusy(reviewId: number, fn: () => Promise<void>) {
  busy.add(reviewId);
  setReview(reviewId, { status: "running" });
  try {
    await fn();
    setReview(reviewId, { status: "done" });
  } catch (e: any) {
    log(reviewId, "error", e.message ?? String(e));
    setReview(reviewId, { status: "error" });
  } finally {
    busy.delete(reviewId);
  }
}

// ---------- setup: find or create the worktree ----------

const SETUP_SCHEMA = {
  type: "object",
  properties: {
    project: { type: "string", description: "project name from the list" },
    pr_number: { type: "integer" },
    worktree_path: { type: "string", description: "absolute path" },
    is_mine: { type: "boolean" },
  },
  required: ["project", "pr_number", "worktree_path", "is_mine"],
};

async function setup(reviewId: number, url: string) {
  const pr = parsePrUrl(url);
  const projects = db.query("SELECT * FROM projects").all() as any[];
  const hint: string[] = [];
  let project: any = null;
  try {
    if (!pr) throw new Error("not a github.com/<owner>/<repo>/pull/<n> URL");
    const remote = new RegExp(`[:/]${pr.owner}/${pr.repo}(\\.git)?/?$`, "i");
    for (const p of projects) {
      const r = await sh(["git", "-C", p.path, "remote", "get-url", "origin"]);
      if (r.ok && remote.test(r.out)) { project = p; break; }
    }
    if (!project) throw new Error(`no project has origin ${pr.owner}/${pr.repo}`);
    hint.push(`project: ${project.name} (${project.path})`);
    const view = JSON.parse(await must(["gh", "pr", "view", url, "--json", "headRefName,author"]));
    hint.push(`headRefName: ${view.headRefName}, author: ${view.author.login}`);
    const wt = findWorktree(await must(["git", "-C", project.path, "worktree", "list", "--porcelain"]), view.headRefName);
    if (!wt) throw new Error(`no worktree has branch ${view.headRefName}`);
    hint.push(`existing worktree: ${wt}`);
    log(reviewId, "tool", `git -C ${wt} pull`);
    await must(["git", "-C", wt, "pull"]);
    return { project_id: project.id, pr_number: pr.number, worktree_path: wt, is_mine: view.author.login === (await ghLogin()) };
  } catch (e: any) {
    hint.push(`failed: ${e.message}`);
    log(reviewId, "tool", `setup check failed (${e.message}), starting setup agent`);
  }

  const prompt = `Set up a git worktree for reviewing ${url}.

Projects (name: path):
${projects.map((p) => `- ${p.name}: ${p.path}`).join("\n") || "(none configured)"}

What the app's own checks found:
${hint.map((h) => `- ${h}`).join("\n")}

Steps:
1. Pick the project whose \`origin\` remote is this PR's repo, and work in its folder.
2. Get the head branch: \`gh pr view ${url} --json headRefName\`.
3. If \`git worktree list\` already has a worktree on that branch, \`git pull\` there and use it.
   Otherwise run /git-workflow:worktree <headRefName>, using <headRefName> exactly as the branch name (no prefix).
4. Inside the worktree run \`gh pr checkout ${pr?.number ?? "<n>"}\` so it holds the PR's real code and upstream.
   If it refuses because the branch diverged, rerun it with \`--force\`.
5. is_mine is true when the PR author login equals \`gh api user --jq .login\`.
Never push, merge, or comment on the PR.`;

  const { structured } = await runClaude(reviewId, prompt, {
    model: "claude-sonnet-5-5",
    cwd: project?.path ?? homedir(),
    settingSources: ["user"],
    allowedTools: ["Skill", "Bash", "Read", "Glob", "Grep"],
    disallowedTools: ["Bash(git push:*)", "Bash(gh pr merge:*)", "Bash(gh pr review:*)", "Bash(gh pr comment:*)"],
    outputFormat: { type: "json_schema", schema: SETUP_SCHEMA },
  });
  if (!structured?.worktree_path || !existsSync(structured.worktree_path))
    throw new Error(`setup agent returned a worktree that does not exist: ${structured?.worktree_path}`);
  const p = projects.find((x) => x.name === structured.project || x.path === structured.project) ?? project;
  return { project_id: p?.id ?? null, pr_number: structured.pr_number, worktree_path: structured.worktree_path, is_mine: !!structured.is_mine };
}

// ---------- review ----------

const OUTPUT_INSTRUCTION = `

---
Finish with only this JSON in a \`\`\`json fence:
{
  "summary": "markdown",
  "findings": [
    {"id": 1, "file": "src/auth.ts", "line": 88, "severity": "high",
     "body": "Token expiry uses \`<\` instead of \`<=\`.",
     "suggested_fix": "if (expiry <= now) {"}
  ]
}
- file is relative to the repo root; line is the line number in the PR's new version of the file.
- severity is high, medium or low.
- suggested_fix is optional. It replaces exactly that one line. Leave it out when the fix spans several lines.`;

async function startReview(reviewId: number, url: string) {
  await withBusy(reviewId, async () => {
    const s = await setup(reviewId, url);
    setReview(reviewId, { ...s, is_mine: s.is_mine ? 1 : 0 });
    const pr = parsePrUrl(url)!;
    const prompt = `${getSetting("prompt", DEFAULT_PROMPT)}\n\nThe PR is ${url} (#${s.pr_number} in ${pr.owner}/${pr.repo}). The current directory is its worktree.${OUTPUT_INSTRUCTION}`;
    let { text } = await runClaude(reviewId, prompt, readOnlyOptions(s.worktree_path));
    let parsed = extractJson(text);
    if (!parsed.value) {
      const retry = await runClaude(reviewId, `your JSON failed: ${parsed.error}, resend only the JSON`, readOnlyOptions(s.worktree_path, review(reviewId).session_id));
      text = retry.text;
      parsed = extractJson(text);
    }
    if (!parsed.value) return setReview(reviewId, { summary: text });
    setReview(reviewId, { summary: String(parsed.value.summary ?? "") });
    saveFindings(reviewId, mergeFindings([], validFindings(parsed.value.findings)));
  });
}

// ---------- chat / fix ----------

async function chat(reviewId: number, text: string) {
  const r = review(reviewId);
  log(reviewId, "user", text);
  await withBusy(reviewId, async () => {
    const out = await runClaude(reviewId, text, readOnlyOptions(r.worktree_path, r.session_id));
    const j = extractJson(out.text).value;
    if (!j) return;
    saveFindings(reviewId, mergeFindings(findings(reviewId), validFindings(j.findings)));
    if (typeof j.summary === "string") setReview(reviewId, { summary: j.summary });
  });
}

const pendingCommit = new Map<number, string>(); // reviewId -> commit message Claude proposed

async function fix(reviewId: number) {
  const r = review(reviewId);
  const chosen = findings(reviewId).filter((f) => f.selected);
  const prompt = `Fix these review findings in the worktree:

${chosen.map((f) => `#${f.id} ${f.file}:${f.line} (${f.severity}): ${f.body}${f.suggested_fix ? `\nSuggested fix: ${f.suggested_fix}` : ""}`).join("\n\n")}

Do not commit or push. When done, end your reply with a commit message in a \`\`\`commit fence.`;
  await withBusy(reviewId, async () => {
    const out = await runClaude(reviewId, prompt, {
      cwd: r.worktree_path, model: model(), resume: r.session_id, settingSources: ["project"],
      permissionMode: "acceptEdits",
      allowedTools: ["Bash"], // acceptEdits does not auto-approve Bash
      disallowedTools: ["Bash(git push:*)", "Bash(git commit:*)", "Bash(gh:*)"], // disallowed wins over allowed
    });
    pendingCommit.set(reviewId, extractCommitMessage(out.text) ?? "Fix review findings");
  });
}

// git diff misses new files, so untracked files are appended as diffs against /dev/null.
async function worktreeDiff(wt: string) {
  const tracked = (await sh(["git", "-C", wt, "diff"])).out;
  const untracked = (await sh(["git", "-C", wt, "ls-files", "--others", "--exclude-standard"])).out.split("\n").filter(Boolean);
  const added = await Promise.all(untracked.map(async (f) => (await sh(["git", "-C", wt, "diff", "--no-index", "/dev/null", f])).out));
  return [tracked, ...added].filter(Boolean).join("\n");
}

// ---------- post ----------

async function post(reviewId: number, event: string) {
  const r = review(reviewId);
  const pr = parsePrUrl(r.pr_url)!;
  const chosen = findings(reviewId).filter((f) => f.selected && !f.posted);
  if (!chosen.length) throw new Error("no selected, unposted findings");
  const payload = buildReview(chosen, diffLines(await must(["gh", "pr", "diff", r.pr_url])), event);
  await must(["gh", "api", `repos/${pr.owner}/${pr.repo}/pulls/${pr.number}/reviews`, "--method", "POST", "--input", "-"], { stdin: JSON.stringify(payload) });
  for (const f of chosen) db.run("UPDATE findings SET posted = 1 WHERE review_id = ? AND id = ?", [reviewId, f.id]);
  log(reviewId, "tool", `posted ${event} review: ${payload.comments.length} inline, ${chosen.length - payload.comments.length} in body`);
  emit({ type: "review", review_id: reviewId });
}

// ---------- HTTP ----------

const json = (v: unknown, status = 200) => Response.json(v, { status });
const fail = (msg: string, status = 400) => json({ error: msg }, status);

function requireIdle(id: number) {
  if (!review(id)) throw Object.assign(new Error("no such review"), { status: 404 });
  if (busy.has(id)) throw Object.assign(new Error("Claude is still running on this review"), { status: 409 });
  return review(id);
}

async function api(req: Request, path: string): Promise<Response> {
  const method = req.method;
  const body: any = method === "GET" ? null : await req.json().catch(() => ({}));
  let m: RegExpMatchArray | null;

  if (path === "/api/events") {
    let ctl: ReadableStreamDefaultController;
    const ping = setInterval(() => { try { ctl.enqueue(enc.encode(": ping\n\n")); } catch {} }, 20000);
    const stream = new ReadableStream({
      start(c) { ctl = c; clients.add(c); },
      cancel() { clients.delete(ctl); clearInterval(ping); },
    });
    return new Response(stream, { headers: { "content-type": "text/event-stream", "cache-control": "no-cache" } });
  }

  if (path === "/api/settings" && method === "GET")
    return json({
      prompt: getSetting("prompt", DEFAULT_PROMPT), model: model(), login: await ghLogin(),
      projects: db.query("SELECT * FROM projects ORDER BY name").all(),
    });
  if (path === "/api/settings" && method === "PUT") {
    for (const k of ["prompt", "model"]) if (typeof body[k] === "string")
      db.run("INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [k, body[k]]);
    return json({ ok: true });
  }
  if (path === "/api/projects" && method === "POST") {
    if (!body.name || !body.path) return fail("name and path are required");
    db.run("INSERT INTO projects (name, path) VALUES (?, ?)", [body.name, body.path]);
    return json({ ok: true });
  }
  if ((m = path.match(/^\/api\/projects\/(\d+)$/))) {
    if (method === "PUT") db.run("UPDATE projects SET name = ?, path = ? WHERE id = ?", [body.name, body.path, Number(m[1])]);
    if (method === "DELETE") db.run("DELETE FROM projects WHERE id = ?", [Number(m[1])]);
    return json({ ok: true });
  }

  if (path === "/api/reviews" && method === "GET")
    return json(db.query("SELECT id, pr_url, pr_number, is_mine, status, created_at FROM reviews ORDER BY id DESC").all());
  if (path === "/api/reviews" && method === "POST") {
    if (!parsePrUrl(body.url ?? "")) return fail("paste a github.com/<owner>/<repo>/pull/<n> URL");
    const id = Number(db.run("INSERT INTO reviews (pr_url, status) VALUES (?, 'running')", [body.url.trim()]).lastInsertRowid);
    emit({ type: "review", review_id: id });
    startReview(id, body.url.trim());
    return json({ id });
  }

  if ((m = path.match(/^\/api\/reviews\/(\d+)$/)) && method === "GET") {
    const id = Number(m[1]);
    const r = review(id);
    if (!r) return fail("no such review", 404);
    const msg = pendingCommit.get(id);
    return json({
      review: r, findings: findings(id), running: busy.has(id),
      messages: db.query("SELECT role, content FROM messages WHERE review_id = ? ORDER BY id").all(id),
      fix: msg !== undefined && r.worktree_path ? { message: msg, diff: await worktreeDiff(r.worktree_path) } : null,
    });
  }

  if ((m = path.match(/^\/api\/reviews\/(\d+)\/findings\/(\d+)$/)) && method === "PATCH") {
    const [rid, fid] = [Number(m[1]), Number(m[2])];
    if (typeof body.selected === "boolean") db.run("UPDATE findings SET selected = ? WHERE review_id = ? AND id = ?", [body.selected ? 1 : 0, rid, fid]);
    if (typeof body.body === "string")
      db.run("UPDATE findings SET body = ?, suggested_fix = ?, edited = 1 WHERE review_id = ? AND id = ?", [body.body, body.suggested_fix || null, rid, fid]);
    return json({ ok: true });
  }

  if ((m = path.match(/^\/api\/reviews\/(\d+)\/(chat|fix|commit|push|post|remove-worktree)$/)) && method === "POST") {
    const id = Number(m[1]);
    const r = requireIdle(id);
    const action = m[2];
    if (!r.worktree_path) return fail("this review has no worktree");
    if (action === "chat") {
      if (!body.text?.trim()) return fail("empty message");
      if (!r.session_id) return fail("the review has no Claude session yet");
      chat(id, body.text.trim());
    } else if (action === "fix") {
      if (!r.is_mine) return fail("fix is only for your own PRs");
      if (!findings(id).some((f) => f.selected)) return fail("select findings first");
      fix(id);
    } else if (action === "commit") {
      if (!body.message?.trim()) return fail("empty commit message");
      await must(["git", "-C", r.worktree_path, "add", "-A"]);
      log(id, "tool", (await must(["git", "-C", r.worktree_path, "commit", "-m", body.message])).split("\n")[0]!);
      pendingCommit.delete(id);
      emit({ type: "review", review_id: id });
    } else if (action === "push") {
      await must(["git", "-C", r.worktree_path, "push"]);
      log(id, "tool", "git push: done");
    } else if (action === "post") {
      if (!["COMMENT", "REQUEST_CHANGES", "APPROVE"].includes(body.event)) return fail("bad event");
      await post(id, body.event);
    } else if (action === "remove-worktree") {
      const common = await must(["git", "-C", r.worktree_path, "rev-parse", "--path-format=absolute", "--git-common-dir"]);
      await must(["git", "--git-dir", common, "worktree", "remove", r.worktree_path]);
      log(id, "tool", `removed worktree ${r.worktree_path}`);
      pendingCommit.delete(id);
      setReview(id, { worktree_path: null });
    }
    return json({ ok: true });
  }

  return fail("not found", 404);
}

Bun.serve({
  hostname: HOST,
  port: PORT,
  idleTimeout: 0, // the default 10s would cut the SSE stream
  async fetch(req) {
    const url = new URL(req.url);
    // Blocks DNS-rebinding pages: the Host header must name this loopback server.
    if (![`${HOST}:${PORT}`, `localhost:${PORT}`].includes(req.headers.get("host") ?? "")) return fail("bad host", 403);
    // Cross-site forms can only send simple content types; requiring JSON forces a CORS preflight, which this server never answers.
    if (req.method !== "GET" && !req.headers.get("content-type")?.startsWith("application/json")) return fail("JSON body required", 415);
    if (url.pathname === "/") return new Response(Bun.file(join(import.meta.dir, "index.html")));
    try {
      return await api(req, url.pathname);
    } catch (e: any) {
      return fail(e.message ?? String(e), e.status ?? 500);
    }
  },
});
console.log(`auto-coder on http://${HOST}:${PORT}`);
