// Phase 1 baseline: how long the server takes to start, to list TODOs and to load one TODO's transcript.
// bun scripts/bench.ts [todos=30] [events=2000]   Uses a throwaway SWITCHBOARD_HOME; nothing of yours is touched.
import { Database } from "bun:sqlite";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { migrateDb } from "../lib";

const N = Number(process.argv[2] ?? 30), EVENTS = Number(process.argv[3] ?? 2000), PORT = 4700 + Math.floor(Math.random() * 200);
const home = mkdtempSync(join(tmpdir(), "sb-bench-"));
const repo = join(home, "repo");
const git = (...a: string[]) => Bun.spawnSync(["git", ...a], { stdout: "ignore", stderr: "ignore" });
git("init", "-q", "-b", "main", repo);
git("-C", repo, "-c", "user.email=b@b", "-c", "user.name=b", "commit", "-q", "--allow-empty", "-m", "init");

const db = new Database(join(home, "db.sqlite"));
migrateDb(db);
const at = new Date().toISOString();
db.run("INSERT INTO projects (id, name, slug, local_path, created_at) VALUES (1, 'bench', 'bench', ?, ?)", [repo, at]);
for (let i = 1; i <= N; i++) {
  const wt = join(home, "wt", String(i));
  git("-C", repo, "worktree", "add", "-q", "-b", `b${i}`, wt);
  db.run("INSERT INTO todos (id, project_id, title, status, worktree_path, created_at, updated_at) VALUES (?, 1, ?, 'idle', ?, ?, ?)", [i, `todo ${i}`, wt, at, at]);
}
const ins = db.prepare("INSERT INTO events (todo_id, kind, actor, detail, at, session_no, meta) VALUES (1, 'tool', 'agent', ?, ?, 1, ?)");
db.transaction(() => { for (let i = 0; i < EVENTS; i++) ins.run(`Read file${i}.ts`, at, JSON.stringify({ name: "Read", status: "completed", input: { file_path: `/x/file${i}.ts` } })); })();
db.close();

const t0 = performance.now();
const server = Bun.spawn(["bun", join(import.meta.dir, "../server.ts")], { env: { ...process.env, SWITCHBOARD_HOME: home, PORT: String(PORT) }, stdout: "ignore", stderr: "inherit", stdin: "ignore" });
const url = `http://127.0.0.1:${PORT}`;
let token = "";
for (let i = 0; i < 400 && !token; i++) { try { await fetch(url); token = readFileSync(join(home, "token"), "utf8").trim(); } catch { await Bun.sleep(10); } }
const startMs = performance.now() - t0;

async function time(path: string, runs = 10) {
  const ms: number[] = [];
  let bytes = 0;
  for (let i = 0; i < runs; i++) {
    const s = performance.now();
    const r = await fetch(url + path, { headers: { "x-switchboard-token": token } });
    bytes = (await r.arrayBuffer()).byteLength;
    ms.push(performance.now() - s);
  }
  ms.sort((a, b) => a - b);
  return `${ms[runs >> 1]!.toFixed(1)} ms median, ${(bytes / 1024).toFixed(0)} KB`;
}
await time("/api/todos", 2);   // warm-up
console.log(`todos=${N} events=${EVENTS}`);
console.log(`server ready      ${startMs.toFixed(0)} ms`);
console.log(`GET /api/todos    ${await time("/api/todos")}`);
console.log(`GET /api/todos/1  ${await time("/api/todos/1")}`);
server.kill("SIGTERM");
await server.exited;
rmSync(home, { recursive: true, force: true });
