import { expect, test } from "bun:test";
import { buildReview, diffLines, extractCommitMessage, extractJson, findWorktree, mergeFindings, parsePrUrl, readOnlyDenyReason, validFindings } from "./lib";

test("parsePrUrl", () => {
  expect(parsePrUrl("https://github.com/acme/api/pull/42/files#diff-1")).toEqual({ owner: "acme", repo: "api", number: 42 });
  expect(parsePrUrl("https://github.com/acme/api/issues/42")).toBeNull();
});

test("findWorktree matches whole branch line", () => {
  const p = "worktree /r\nHEAD a\nbranch refs/heads/main\n\nworktree /w/feature-2\nHEAD b\nbranch refs/heads/feature-2\n\nworktree /w/feature\nHEAD c\nbranch refs/heads/feature\n";
  expect(findWorktree(p, "feature")).toBe("/w/feature");
  expect(findWorktree(p, "nope")).toBeNull();
});

test("extractJson picks fenced findings JSON, reports errors", () => {
  expect(extractJson('text\n```json\n{"summary":"s","findings":[]}\n```').value.summary).toBe("s");
  expect(extractJson('{"summary":"s","findings":[]}').value.findings).toEqual([]);
  expect(extractJson("```json\n{bad\n```").error).toBeTruthy();
});

test("validFindings drops incomplete", () => {
  const f = validFindings([{ id: 1, file: "a", line: 2, severity: "HIGH", body: "x" }, { id: 2, file: "a", severity: "low", body: "x" }]);
  expect(f.map((x) => x.id)).toEqual([1]);
  expect(f[0]!.severity).toBe("high");
});

test("mergeFindings keeps selected and edited body", () => {
  const old = [{ id: 1, file: "a", line: 1, severity: "high", body: "mine", selected: 1, edited: 1 },
               { id: 2, file: "b", line: 1, severity: "low", body: "old", selected: 1, edited: 0 }];
  const next = validFindings([{ id: 2, file: "b", line: 1, severity: "low", body: "new" }, { id: 1, file: "a", line: 1, severity: "high", body: "claude" }]);
  const m = mergeFindings(old, next);
  expect(m.find((f) => f.id === 1)).toMatchObject({ body: "mine", selected: 1 });
  expect(m.find((f) => f.id === 2)).toMatchObject({ body: "new", selected: 1 });
});

test("diffLines + buildReview", () => {
  const diff = "diff --git a/x.ts b/x.ts\n--- a/x.ts\n+++ b/x.ts\n@@ -10,3 +10,4 @@ fn\n ctx\n-old\n+new\n+new2\n ctx\n";
  const lines = diffLines(diff);
  expect([...lines.get("x.ts")!]).toEqual([10, 11, 12, 13]);
  const r = buildReview([
    { id: 1, file: "x.ts", line: 11, severity: "high", body: "b", suggested_fix: "fix()" },
    { id: 2, file: "x.ts", line: 99, severity: "low", body: "far" },
  ], lines, "COMMENT");
  expect(r.comments).toEqual([{ path: "x.ts", line: 11, side: "RIGHT", body: "b\n\n```suggestion\nfix()\n```" }]);
  expect(r.body).toContain("x.ts:99");
});

test("readOnlyDenyReason", () => {
  expect(readOnlyDenyReason("Read", {})).toBeNull();
  expect(readOnlyDenyReason("Edit", {})).toBeTruthy();
  expect(readOnlyDenyReason("Bash", { command: "git diff main...HEAD" })).toBeNull();
  expect(readOnlyDenyReason("Bash", { command: "gh api repos/a/b/pulls/1/comments --paginate" })).toBeNull();
  expect(readOnlyDenyReason("Bash", { command: "gh api -X POST repos/a/b/issues/1/comments" })).toBeTruthy();
  expect(readOnlyDenyReason("Bash", { command: "gh api repos/a/b/issues/1/comments -f body=hi" })).toBeTruthy();
  expect(readOnlyDenyReason("Bash", { command: "git diff && git commit -m x" })).toBeTruthy();
  expect(readOnlyDenyReason("Bash", { command: "git log > out.txt" })).toBeTruthy();
});

test("extractCommitMessage", () => {
  expect(extractCommitMessage("done\n```commit\nfix: x\n\nbody\n```")).toBe("fix: x\n\nbody");
});
