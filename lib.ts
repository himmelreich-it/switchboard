// Pure helpers. No I/O here, so lib.test.ts can cover them.
import { isAbsolute, relative } from "node:path";

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
