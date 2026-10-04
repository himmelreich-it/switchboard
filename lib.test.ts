import { expect, test } from "bun:test";
import {
  cutToTwoLines, deliveryAction, deliveryPrompt, isInside, lineCount, safeFileName, slugify, statusAfterRun, toolArg, toolLine,
  uniqueSlug, withAttachments,
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
