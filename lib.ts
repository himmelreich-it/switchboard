// Pure helpers. No I/O here, so lib.test.ts can cover them.

export type Finding = {
  id: number;
  file: string;
  line: number;
  severity: string;
  body: string;
  suggested_fix?: string | null;
  selected?: number;
  posted?: number;
  edited?: number;
};

// github.com/<owner>/<repo>/pull/<n>, ignoring a trailing /files, /commits, ?query or #anchor.
export function parsePrUrl(url: string) {
  const m = url.trim().match(/^https?:\/\/github\.com\/([^/]+)\/([^/]+)\/pull\/(\d+)(?:[/?#].*)?$/);
  if (!m) return null;
  return { owner: m[1]!, repo: m[2]!, number: Number(m[3]) };
}

// Finds the worktree path whose porcelain block has exactly "branch refs/heads/<branch>".
export function findWorktree(porcelain: string, branch: string): string | null {
  for (const block of porcelain.split(/\n\n+/)) {
    const lines = block.split("\n");
    if (lines.includes(`branch refs/heads/${branch}`)) {
      const wt = lines.find((l) => l.startsWith("worktree "));
      if (wt) return wt.slice("worktree ".length);
    }
  }
  return null;
}

// Returns the last JSON object in the text that has a "findings" array (fenced ```json first, then bare braces).
export function extractJson(text: string): { value?: any; error?: string } {
  const fences = [...text.matchAll(/```(?:json)?\s*\n([\s\S]*?)```/g)].map((m) => m[1]!);
  const first = text.indexOf("{");
  const last = text.lastIndexOf("}");
  const candidates = [...fences.reverse(), first >= 0 && last > first ? text.slice(first, last + 1) : ""];
  let error = "no JSON object found";
  for (const c of candidates) {
    if (!c.trim()) continue;
    try {
      const value = JSON.parse(c);
      if (value && typeof value === "object" && Array.isArray(value.findings)) return { value };
      error = 'JSON has no "findings" array';
    } catch (e: any) {
      error = e.message;
    }
  }
  return { error };
}

// Drops findings missing file, line, severity or body.
export function validFindings(list: any[]): Finding[] {
  return list
    .filter((f) => f && typeof f.file === "string" && f.file && Number.isInteger(Number(f.line)) && Number(f.line) > 0
      && typeof f.severity === "string" && f.severity && typeof f.body === "string" && f.body)
    .map((f, i) => ({
      id: Number.isInteger(Number(f.id)) ? Number(f.id) : i + 1,
      file: f.file,
      line: Number(f.line),
      severity: f.severity.toLowerCase(),
      body: f.body,
      suggested_fix: typeof f.suggested_fix === "string" && f.suggested_fix ? f.suggested_fix : null,
    }));
}

// New list wins; a finding with the same id keeps selected, posted, and the body if the user edited it.
export function mergeFindings(old: Finding[], next: Finding[]): Finding[] {
  const byId = new Map(old.map((f) => [f.id, f]));
  return next.map((f) => {
    const o = byId.get(f.id);
    if (!o) return { ...f, selected: 0, posted: 0, edited: 0 };
    return {
      ...f,
      selected: o.selected ?? 0,
      posted: o.posted ?? 0,
      edited: o.edited ?? 0,
      body: o.edited ? o.body : f.body,
      suggested_fix: o.edited ? o.suggested_fix : f.suggested_fix,
    };
  });
}

// Maps each file in a unified diff to the new-side line numbers GitHub accepts for inline comments.
export function diffLines(diff: string): Map<string, Set<number>> {
  const files = new Map<string, Set<number>>();
  let cur: Set<number> | null = null;
  let line = 0;
  for (const l of diff.split("\n")) {
    if (l.startsWith("+++ ")) {
      const path = l.slice(4).replace(/^b\//, "");
      cur = path === "/dev/null" ? null : new Set();
      if (cur) files.set(path, cur);
      continue;
    }
    const h = l.match(/^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
    if (h) { line = Number(h[1]); continue; }
    if (!cur || l.startsWith("--- ") || l.startsWith("diff ") || l.startsWith("\\")) continue;
    if (l.startsWith("+") || l.startsWith(" ")) cur.add(line++);
  }
  return files;
}

export function commentBody(f: Finding): string {
  return f.suggested_fix ? `${f.body}\n\n\`\`\`suggestion\n${f.suggested_fix}\n\`\`\`` : f.body;
}

// Splits findings into inline comments (line is in the diff) and body lines (line is not).
export function buildReview(findings: Finding[], lines: Map<string, Set<number>>, event: string) {
  const comments: { path: string; line: number; side: "RIGHT"; body: string }[] = [];
  const outside: string[] = [];
  for (const f of findings) {
    if (lines.get(f.file)?.has(f.line)) comments.push({ path: f.file, line: f.line, side: "RIGHT", body: commentBody(f) });
    else outside.push(`**${f.file}:${f.line}** (${f.severity})\n\n${commentBody(f)}`);
  }
  return { event, body: outside.join("\n\n---\n\n"), comments };
}

const READ_ONLY_BASH = ["git diff", "git log", "gh pr view", "gh pr diff", "gh api"];
const READ_ONLY_TOOLS = ["Read", "Grep", "Glob"];

// Why a review/chat tool call is refused, or null when allowed.
// Runs as a PreToolUse hook, because allow rules skip canUseTool entirely.
export function readOnlyDenyReason(tool: string, input: any): string | null {
  if (READ_ONLY_TOOLS.includes(tool)) return null;
  if (tool !== "Bash") return `${tool} is not allowed in a read-only review`;
  const cmd = String(input?.command ?? "");
  if (/[`$<>]/.test(cmd)) return "command substitution and redirects are not allowed";
  // ponytail: splits on shell operators with a regex; quoted "&&" inside an argument is refused too, which is the safe side.
  for (const part of cmd.split(/&&|\|\||[;|\n]/).map((p) => p.trim()).filter(Boolean)) {
    if (!READ_ONLY_BASH.some((p) => part === p || part.startsWith(p + " "))) return `only ${READ_ONLY_BASH.join(", ")} are allowed`;
    if (part.startsWith("gh api") && /(^|\s)(-[XfF]|--method|--field|--raw-field|--input)/.test(part))
      return "gh api with -X, --method, -f, -F or --input writes to GitHub";
  }
  return null;
}

// Text between ```commit fences, used as the fix commit message.
export function extractCommitMessage(text: string): string | null {
  const m = [...text.matchAll(/```commit\s*\n([\s\S]*?)```/g)].pop();
  return m ? m[1]!.trim() : null;
}
