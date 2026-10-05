// Pure helpers. No I/O here, so lib.test.ts can cover them.
import { basename, isAbsolute, relative } from "node:path";

export type Status = "not_started" | "running" | "waiting" | "idle";

// "Billing API!" -> "billing-api". Used once, when a project is created; the slug names its artifact folder.
export function slugify(name: string): string {
  return name.toLowerCase().normalize("NFKD").replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "") || "project";
}

// The slug itself if free, else slug-2, slug-3, ...
export function uniqueSlug(slug: string, taken: Set<string>): string {
  let s = slug;
  for (let n = 2; taken.has(s); n++) s = `${slug}-${n}`;
  return s;
}

// A turn's final text shown as a chat message: at most two lines and 280 characters,
// because one long paragraph is a single line too.
export function cutToTwoLines(text: string): string {
  const lines = text.trim().split("\n").map((l) => l.trim()).filter(Boolean);
  let out = lines.slice(0, 2).join("\n");
  if (out.length > 280) out = out.slice(0, 279).trimEnd();
  return out.length < lines.join("\n").length ? `${out}…` : out;
}

export const lineCount = (text: string) => text.trim().split("\n").filter((l) => l.trim()).length;

// The prompt for one turn: the worktree line (while one is registered), then every delivered message.
// With no messages, as on a restore with nothing queued and no draft, the prompt is "continue".
export function deliveryPrompt(worktree: string | null, bodies: string[]): string {
  const text = bodies.map((b) => b.trim()).filter(Boolean).join("\n\n") || "continue";
  return worktree ? `Your worktree: ${worktree}. Work there.\n\n${text}` : text;
}

// One message as the agent gets it: the text, then the absolute paths of the files the user attached to it.
export function withAttachments(body: string, paths: string[]): string {
  if (!paths.length) return body;
  return [body.trim(), `Attached files (open with Read):\n${paths.join("\n")}`].filter(Boolean).join("\n\n");
}

// "../My Shot (1).png" -> "My-Shot-1-.png": an uploaded file's name as one safe path segment, at most 120 characters
// (the end is kept, so the extension stays).
export function safeFileName(name: string): string {
  return basename(name).replace(/[^A-Za-z0-9._-]+/g, "-").slice(-120).replace(/^[.-]+/, "") || "file";
}

// What a committed message does, by the TODO's status:
// not started -> stored until start; running -> queued until the run ends; idle or waiting -> sent now.
export function deliveryAction(status: Status): "pending" | "queue" | "send" {
  return status === "not_started" ? "pending" : status === "running" ? "queue" : "send";
}

// A run that ended (normally, with an error, or by interrupt) leaves the TODO waiting while the flag is raised.
export const statusAfterRun = (attention: boolean | number): Status => (attention ? "waiting" : "idle");

// True when `path` is inside `dir` (not `dir` itself). Both must already be absolute and resolved.
export function isInside(path: string, dir: string): boolean {
  const rel = relative(dir, path);
  return rel !== "" && !rel.startsWith("..") && !isAbsolute(rel);
}

// The interesting argument of a tool call: the Bash command, the file path, the search pattern...
export const toolArg = (input: any): string =>
  String(input?.command ?? input?.file_path ?? input?.pattern ?? input?.skill ?? input?.path ?? input?.url ?? "").slice(0, 300);

// "Bash git status", "Read /a/b.ts": one line per tool call for the events list.
export const toolLine = (name: string, input: any) => `${name} ${toolArg(input)}`.trim();

// ---------- git and GitHub (the card's git line and PR line) ----------

// `git status --porcelain=v2 --branch` -> the branch and how many files changed or are untracked.
// Changed counts ordinary (1), renamed (2) and unmerged (u) entries; "(detached)" means no branch.
export function parseGitStatus(out: string) {
  let branch: string | null = null, changed = 0, untracked = 0;
  for (const line of out.split("\n")) {
    if (line.startsWith("# branch.head ")) branch = line.slice(14) === "(detached)" ? null : line.slice(14);
    else if (/^[12u] /.test(line)) changed++;
    else if (line.startsWith("? ")) untracked++;
  }
  return { branch, changed, untracked };
}

// `git for-each-ref --format='%(refname) %(symref:short) %(ahead-behind:HEAD)' refs/remotes/origin/HEAD refs/remotes/origin/<branch>`:
//   refs/remotes/origin/HEAD origin/main 5 7       -> 5 behind main, 7 commits on the branch
//   refs/remotes/origin/<branch>  0 1              -> 1 commit not pushed; no such line -> the branch was never pushed
// No origin/HEAD line leaves base, behind and commits null (unknown, not zero).
export function parseRefCounts(out: string, branch: string | null) {
  let base: string | null = null, behind: number | null = null, commits: number | null = null, unpushed: number | null = null;
  for (const line of out.split("\n")) {
    const parts = line.trim().split(" ");
    if (parts.length < 3) continue;
    const theirs = Number(parts.at(-2)), mine = Number(parts.at(-1));
    if (parts[0] === "refs/remotes/origin/HEAD") [base, behind, commits] = [parts[1]!.replace(/^origin\//, "") || null, theirs, mine];
    else if (branch && parts[0] === `refs/remotes/origin/${branch}`) unpushed = mine;
  }
  const push = !branch ? null : unpushed === null ? "none" : unpushed ? "unpushed" : "pushed";
  return { base, behind, commits, push, unpushed: unpushed ?? 0 };
}

// "git@github.com:Hostaway/copilot-service.git" or "https://github.com/himmelreich-it/switchboard.git" -> owner and name.
export function parseGithubRemote(url: string) {
  const m = url.trim().match(/github\.com[:/]([^/\s]+)\/([^/\s]+?)(?:\.git)?\/?$/);
  return m ? { owner: m[1], name: m[2] } : null;
}

const PR_FIELDS = `fragment F on PullRequest { number url state isDraft mergeable mergeStateStatus reviewDecision baseRefName
  reviewThreads(first: 100) { nodes { isResolved } }
  commits(last: 1) { nodes { commit { statusCheckRollup { state contexts(first: 100) { nodes {
    __typename ... on CheckRun { name status conclusion } ... on StatusContext { context state } } } } } } } }`;

// One GraphQL query for a whole repository: alias b<i> finds the newest PR whose head is branch $b<i>,
// alias n<i> reads PR number $n<i> (registered with set_pr). Values go in as variables, never into the query text.
export function prQuery(branches: number, numbers: number) {
  const vars = ["$o: String!", "$n: String!"], parts: string[] = [];
  for (let i = 0; i < branches; i++) {
    vars.push(`$b${i}: String!`);
    parts.push(`b${i}: pullRequests(headRefName: $b${i}, first: 1, orderBy: {field: CREATED_AT, direction: DESC}) { nodes { ...F } }`);
  }
  for (let i = 0; i < numbers; i++) {
    vars.push(`$n${i}: Int!`);
    parts.push(`n${i}: pullRequest(number: $n${i}) { ...F }`);
  }
  return `query(${vars.join(", ")}) { repository(owner: $o, name: $n) { ${parts.join(" ")} } }\n${PR_FIELDS}`;
}

export type Pr = {
  number: number; url: string; state: "open" | "merged" | "closed"; draft: boolean; conflicts: boolean; mergeable: boolean;
  behind: boolean; base: string; review: "approved" | "changes" | "required" | null; threads: number;
  checks: { state: "failing" | "running" | "passing" | null; failing: string[]; running: number; passed: number; skipped: number };
};
const REVIEW: Record<string, Pr["review"]> = { APPROVED: "approved", CHANGES_REQUESTED: "changes", REVIEW_REQUIRED: "required" };
const ROLLUP: Record<string, Pr["checks"]["state"]> = { FAILURE: "failing", ERROR: "failing", PENDING: "running", EXPECTED: "running", SUCCESS: "passing" };

// One PullRequest node of prQuery() in the shape the card reads. The overall check state is GitHub's rollup;
// the counts and failing names come from the single checks. mergeable UNKNOWN (not computed yet) is neither.
export function prFromNode(n: any): Pr {
  const rollup = n.commits?.nodes?.[0]?.commit?.statusCheckRollup;
  const checks: Pr["checks"] = { state: ROLLUP[rollup?.state] ?? null, failing: [], running: 0, passed: 0, skipped: 0 };
  for (const c of rollup?.contexts?.nodes ?? []) {
    const result = c.__typename === "StatusContext" ? c.state : c.status === "COMPLETED" ? c.conclusion : "PENDING";
    if (result === "SUCCESS" || result === "NEUTRAL") checks.passed++;
    else if (result === "SKIPPED") checks.skipped++;
    else if (result === "PENDING" || result === "EXPECTED") checks.running++;
    else checks.failing.push(c.name ?? c.context);
  }
  return {
    number: n.number, url: n.url, state: String(n.state).toLowerCase() as Pr["state"], draft: !!n.isDraft,
    conflicts: n.mergeable === "CONFLICTING", mergeable: n.mergeable === "MERGEABLE", behind: n.mergeStateStatus === "BEHIND",
    base: n.baseRefName, review: REVIEW[n.reviewDecision] ?? null,
    threads: (n.reviewThreads?.nodes ?? []).filter((t: any) => !t.isResolved).length, checks,
  };
}

// How one `gh api graphql` call went. gh exits 1 on any GraphQL error but still prints the data, so a bad set_pr number
// (error on path repository.n0) keeps the other PRs. An error on the repository itself, or a rejected token (HTTP 401),
// means this account cannot see it, and the next account is tried.
export function ghOutcome(code: number | null, stdout: string, stderr: string):
  { repo: any } | { error: "no access" | "no network" | "unclear"; detail: string } {
  let j: any = null;
  try { j = JSON.parse(stdout); } catch {}
  if (j?.errors?.some((e: any) => e.path?.length === 1 && e.path[0] === "repository") || /HTTP 401/.test(stderr)) return { error: "no access", detail: "" };
  if (j?.data?.repository) return { repo: j.data.repository };
  const text = (stderr || stdout).trim();
  if (/dial tcp|error connecting to|internet connection|no such host|i\/o timeout|TLS handshake/i.test(text)) return { error: "no network", detail: text };
  return { error: "unclear", detail: `exit ${code ?? "killed"}${text ? " · " + text.split("\n")[0]!.slice(0, 160) : ""}` };
}
