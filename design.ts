// Design review (item 151): the TODO's agent (the author) writes a design, another agent reviews it in a child TODO, and
// the app relays between the two until they agree, the round cap is hit, or a rejected finding keeps coming back. The
// tools (apptools.ts) only record a request or a review; the turn's end (runner.ts endRun) relays it, because starting
// the other side from inside a tool call would close the caller mid-turn. The rules themselves are in lib.ts.
import { basename } from "node:path";
import { DEFAULT_AGENT, agentList, agentName } from "./agents";
import {
  DESIGN_MAX, DESIGN_START, agreedText, answersError, authorRevise, newDesign, pickReviewer, reviewOutcome, reviewerFirst, reviewerNext, stuckText,
  type Answer, type Design, type DesignConfig, type Finding, type ReviewerOf,
} from "./lib";
// server.ts and runner.ts import this file too (an ES module cycle): use these only inside functions, never at the top level.
import { commitMessage, deliver, isRunning, start } from "./runner";
import { addEvent, addMessage, createTodo, db, filesOf, projectRow, setTodo, todoRow, type Todo } from "./server";

const parse = (t: { design: string | null }) => (t.design ? JSON.parse(t.design) as Design | ReviewerOf : null);
// The author's design state; null for a TODO without design review and for a reviewer.
export const authorDesign = (t: { design: string | null }) => { const d = parse(t); return d && !("of" in d) ? d : null; };
// The author TODO a reviewer TODO reviews for; null for any other TODO.
export const reviewerOf = (t: { design: string | null }) => { const d = parse(t); return d && "of" in d ? d.of : null; };
const save = (id: number, d: Design) => setTodo(id, { design: JSON.stringify(d) }, false);
export const designConfig = (projectId: number | null): DesignConfig | null => {
  const raw = projectRow(projectId)?.design;
  return raw ? JSON.parse(raw) : null;
};
const reviewerAgent = (t: Todo) => pickReviewer(designConfig(t.project_id), t.agent || DEFAULT_AGENT, agentList().filter((a) => a.found).map((a) => a.id));

// Raises attention on a TODO the app does not run right now (the other side of the loop).
function raise(id: number, why: string) {
  const t = todoRow(id)!;
  if (!t.attention) addEvent(id, "attention raised", "app", why);
  setTodo(id, { attention: 1, attention_by: t.attention ? t.attention_by : "app", ...(isRunning(id) || t.status === "not_started" ? {} : { status: "waiting" }) });
}

// Turns design review on: from New TODO, routing, the operator or the TODO's own agent. Before session 1 the author's
// instruction waits for the start; later it is queued and goes out now, or when the current turn ends.
// Returns why it was refused, or null.
export function startDesign(todoId: number, by: "user" | "operator" | "agent"): string | null {
  const t = todoRow(todoId);
  if (!t || t.removed_at) return `no live TODO T-${todoId}`;
  if (reviewerOf(t)) return `T-${todoId} is a design reviewer itself.`;
  if (authorDesign(t)?.phase === "design") return `Design review runs already in T-${todoId}.`;
  const found = agentList().filter((a) => a.found);
  if (t.routed_at ? !reviewerAgent(t) : found.length < 2) return "Design review needs a second agent: Settings › Agents.";
  save(todoId, newDesign());
  addMessage(todoId, "app", DESIGN_START, t.session_id ? "queued" : "pending");
  addEvent(todoId, "design review", by, "on");
  if (t.session_id) deliver(todoId, "design review");
  return null;
}

// request_review: the author's design file (attached to its TODO) and its answers to the last review's blocking findings.
export function requestReview(todoId: number, fileName: string, answers: Answer[]): string | { error: string } {
  const t = todoRow(todoId)!, d = authorDesign(t);
  if (!d) return { error: "Design review is not on for this TODO." };
  const f = filesOf(todoId).find((x) => basename(x.path) === basename(fileName));
  if (!f) return { error: `No attached file is named ${fileName}. Call attach_file first.` };
  if (!d.reviewer && !reviewerAgent(t)) return { error: "No second agent to review it: the user adds one in Settings › Agents." };
  const e = answersError(d.phase === "agreed" ? [] : d.open, answers);
  if (e) return { error: e };
  save(todoId, { ...d, request: { file: f.path, answers } });
  addEvent(todoId, "request_review", "agent", basename(f.path));
  return "Recorded. End your turn now: the app sends the design to the reviewer.";
}

// submit_review: the reviewer's verdict, recorded on the author TODO.
export function submitReview(todoId: number, verdict: "approve" | "changes", findings: Finding[], fileName?: string): string | { error: string } {
  const of = reviewerOf(todoRow(todoId)!);
  const p = of ? todoRow(of) : null, d = p && authorDesign(p);
  if (!d) return { error: "This TODO is not a design reviewer." };
  if (d.phase !== "design" || d.waiting !== "reviewer") return { error: `T-${of} does not wait for a review now.` };
  const ids = findings.map((f) => f.id.trim());
  if (ids.some((x) => !x) || new Set(ids).size !== ids.length) return { error: "Each finding needs its own id (F1, F2, …)." };
  let file: string | null = null;
  if (fileName) {
    const f = filesOf(todoId).find((x) => basename(x.path) === basename(fileName));
    if (!f) return { error: `No attached file is named ${fileName}. Call attach_file first.` };
    file = f.path;
  }
  save(of!, { ...d, review: { verdict, findings: findings.map((f) => ({ ...f, id: f.id.trim() })), file } });
  const blocking = findings.filter((f) => f.severity === "blocking").length;
  addEvent(todoId, "submit_review", "agent", `${verdict} · ${blocking} blocking · ${findings.length - blocking} nit`);
  return "Recorded. End your turn now: the app sends the review to the author.";
}

// The end of a worker turn that was not stopped and did not fail: relays what the turn recorded.
export function designStep(todoId: number) {
  const t = todoRow(todoId)!;
  const of = reviewerOf(t);
  if (of) return reviewed(of, t);
  const d = authorDesign(t);
  if (!d || d.phase !== "design") return;
  if (d.request) return sendForReview(t, d);
  if (d.waiting === "author") nudge(t, d, todoId, "The design review waits for you: call request_review with your design file, then end your turn.");
}

// A turn of the side the loop waits for ended without its tool call: the app asks once, then raises attention once.
// Not while the user talks to it (attention raised, messages queued).
function nudge(author: Todo, d: Design, side: number, text: string) {
  const t = todoRow(side)!;
  if (t.attention || db.query("SELECT 1 FROM messages WHERE todo_id = ? AND state = 'queued'").get(side)) return;
  save(author.id, { ...d, nudged: d.nudged + 1 });
  if (d.nudged === 0) addMessage(side, "app", text, "queued");
  else if (d.nudged === 1) {
    addMessage(author.id, "app", `Design review paused: ${side === author.id ? "the agent" : `the reviewer in T-${side}`} ended its turn without ${side === author.id ? "request_review" : "submit_review"}.`);
    raise(author.id, "design review paused");
  }
}

function sendForReview(t: Todo, d: Design) {
  const req = d.request!, round = d.round + 1;
  // The topic's cap on the first round; after agreed or stuck, a request is one more round.
  const max = d.round === 0 ? designConfig(t.project_id)?.max ?? DESIGN_MAX : d.phase === "design" ? d.max : round;
  const next: Design = { ...d, phase: "design", round, max, file: req.file, request: null, review: null, waiting: "reviewer", nudged: 0,
    rejected: req.answers.filter((a) => a.answer === "rejected").map((a) => a.id) };
  const live = d.reviewer ? todoRow(d.reviewer) : null;
  let reviewer = d.reviewer!;
  if (!live || live.removed_at) {
    const agent = reviewerAgent(t);
    if (!agent) {
      save(t.id, { ...d, request: null });
      addMessage(t.id, "app", "Design review needs a second agent: Settings › Agents.");
      return raise(t.id, "no reviewer agent");
    }
    reviewer = createTodo(reviewerFirst({ parent: t.id, title: t.title, file: req.file, worktree: t.worktree_path, folder: projectRow(t.project_id)?.local_path ?? null }),
      "agent", [], agent, { parent_id: t.id, project_id: t.project_id, title: `Design review of T-${t.id}` });
    setTodo(reviewer, { design: JSON.stringify({ of: t.id } satisfies ReviewerOf), worktree_path: t.worktree_path }, false);
    save(t.id, { ...next, reviewer });
    start(reviewer);
  } else {
    save(t.id, next);
    commitMessage(reviewer, "user via operator", reviewerNext({ round, file: req.file, answers: req.answers }), [], `T-${t.id}`);
  }
  addEvent(t.id, "design review", "app", `round ${round} of ${max} · sent to T-${reviewer}`);
  addMessage(t.id, "app", `Design sent for review, round ${round} of ${max}: T-${reviewer} (${agentName(todoRow(reviewer)!.agent)}).`);
}

function reviewed(authorId: number, rt: Todo) {
  const p = todoRow(authorId), d = p && authorDesign(p);
  if (!p || p.removed_at || !d || d.phase !== "design" || d.waiting !== "reviewer" || d.reviewer !== rt.id) return;
  if (!d.review) return nudge(p, d, rt.id, "Call submit_review with your verdict and findings, then end your turn.");
  const r = d.review, o = reviewOutcome(d, r);
  const next: Design = { ...d, review: null, open: o.open, bounces: o.bounces, waiting: "author", nudged: 0 };
  addEvent(authorId, "design review", "app", `round ${d.round} · ${o.outcome} · ${o.open.length} blocking`);
  if (o.outcome === "agreed") {
    const implement = designConfig(p.project_id)?.after === "implement";
    save(authorId, { ...next, phase: "agreed" });
    addMessage(authorId, "app", `${agreedText(d.round, implement)} Design: ${d.file}`);
    if (implement) {
      addMessage(authorId, "app", "The design is agreed. Implement it now.", "queued");
      return deliver(authorId, "design agreed");
    }
    setTodo(authorId, { suggestions: JSON.stringify(["Implement the design", "One more review round"]) }, false);
    return raise(authorId, "design agreed");
  }
  if (o.outcome === "stuck") {
    save(authorId, { ...next, phase: "stuck" });
    addMessage(authorId, "app", stuckText(d, o.open));
    setTodo(authorId, { suggestions: JSON.stringify(["Continue review"]) }, false);
    return raise(authorId, "design review stuck");
  }
  save(authorId, next);
  commitMessage(authorId, "user via operator", authorRevise({ round: d.round, max: d.max, reviewer: agentName(rt.agent), review: r }), [], `T-${rt.id}`);
}
