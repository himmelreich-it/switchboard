import { expect, test } from "bun:test";
import {
  cutToTwoLines, deliveryAction, deliveryPrompt, ghOutcome, isInside, lineCount, parseGitStatus, parseGithubRemote, parseRefCounts,
  prFromNode, safeFileName, slugify, statusAfterRun, toolArg, toolLine, uniqueSlug, withAttachments,
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
