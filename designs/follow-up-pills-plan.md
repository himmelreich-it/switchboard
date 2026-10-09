# Suggested follow-ups (pills) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The worker agent suggests up to two follow-ups with a new app tool. They show as pills in the TODO's chat and on its board card, and a click sends one as the user's message.

**Architecture:** A new worker MCP tool `suggest_follow_ups` stores the items in `todos.suggestions` (JSON). Every run start and every committed message clears them. `summaries()` hands them to the page, which renders pills under the same conditions in `renderChat` and `cardHtml`. A click posts to the existing messages route with `suggested: true`.

**Tech Stack:** Bun, TypeScript (server: `server.ts`, `runner.ts`, `apptools.ts`, pure helpers in `lib.ts`), `bun:test`, a single-file page `index.html` (vanilla JS, no build).

**Spec:** `designs/follow-up-pills.md` (RESOLVED). Feature spec: `designs/feature-spec-switchboard.md`.

## Global Constraints

- 1–2 items per call, each one line, ≤ 80 characters, not empty.
- The tool exists for the TODO's agent only, not for Claude terminals (grants with `via`).
- A click sends a plain `user` message. There is no new author, and delivery is unchanged (item 20).
- Pills show only when the TODO is not `running` and has no open permission request.
- The operator's chat gets no pills.
- Never commit to `main`: work on branch `design/follow-up-pills` in `C:\code\worktrees\follow-up-pills`.
- Write code comments sparingly, in the style of the surrounding code (short, item numbers where the spec has one).

## Review Focus

1. **A plain click while a draft is in the composer.** The draft must survive. The messages route clears the draft today (`setDraft(`todo:${id}`, "")`), so it must skip that for `suggested`. Pinned in Task 2, Step 6 (manual check in Task 4).
2. **A turn that does not call the tool.** It must show no pills left over from an earlier turn. The clear goes in both `runWorker` and `unprompted` (the background-task follow-up run). Pinned in Task 4's manual check.
3. **A pill clicked while the agent started running in another tab.** The message is queued like any message, and the pills disappear. This needs no code beyond `commitMessage` clearing them; covered by Task 2, Step 4.
4. **Pill text containing HTML or quotes** (`Use <b> tag`, `Say "yes"`). The text must be escaped in both the label and the `title`. The click reads the text from `S.todos` by index, never from the DOM. Pinned in Task 3, Step 2.
5. **A terminal Claude calling the tool.** The tool must not be offered: it is registered only without `via`, and it is filtered out of the terminal's `--allowedTools`. Pinned in Task 2, Step 2.

---

### Task 1: Validation helper and column

**Files:**
- Modify: `lib.ts` (next to `lineCount`, line 28; `migrateDb`, after the last `addColumn`)
- Modify: `server.ts:29-35` (`Todo` type)
- Test: `lib.test.ts`

**Interfaces:**
- Produces: `export function followUpError(items: string[]): string | null`, which returns the error text, or `null` when the items are valid. It also adds the column `todos.suggestions TEXT` and the field `suggestions: string | null` on `Todo`.

- [ ] **Step 1: Write the failing tests** (append to `lib.test.ts`, and add `followUpError` to the import list from `./lib`)

```ts
test("followUpError", () => {
  expect(followUpError(["Approve the PR"])).toBeNull();
  expect(followUpError(["Approve the PR", "Post the two nits as review comments"])).toBeNull();
  expect(followUpError([])).toBe("Suggest one or two follow-ups.");
  expect(followUpError(["a", "b", "c"])).toBe("Suggest one or two follow-ups.");
  expect(followUpError(["  "])).toBe("A follow-up is empty.");
  expect(followUpError(["one\ntwo"])).toBe("A follow-up is one line.");
  expect(followUpError(["x".repeat(81)])).toBe("A follow-up is at most 80 characters.");
  expect(followUpError(["x".repeat(80)])).toBeNull();
});

test("migrateDb adds todos.suggestions", () => {
  const { Database } = require("bun:sqlite");   // as the existing migrateDb test does
  const db = new Database(":memory:");
  migrateDb(db);
  expect(db.query("SELECT 1 FROM pragma_table_info('todos') WHERE name = 'suggestions'").get()).toBeTruthy();
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `bun test lib.test.ts`
Expected: FAIL. `followUpError` is not exported, and the migrate test fails on the missing column.

- [ ] **Step 3: Implement**

In `lib.ts`, after `lineCount`:

```ts
// The agent's suggested follow-ups (pills): one or two one-line instructions the user sends with a click.
export function followUpError(items: string[]): string | null {
  if (items.length < 1 || items.length > 2) return "Suggest one or two follow-ups.";
  for (const s of items) {
    if (!s.trim()) return "A follow-up is empty.";
    if (s.trim().includes("\n")) return "A follow-up is one line.";
    if (s.trim().length > 80) return "A follow-up is at most 80 characters.";
  }
  return null;
}
```

In `migrateDb`, after the last `addColumn`:

```ts
  addColumn("todos", "suggestions", "TEXT");     // the agent's suggested follow-ups (JSON string[]), cleared at each run start
```

In `server.ts`, in the `Todo` type, add `suggestions: string | null;` after `agent: string | null;`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `bun test`
Expected: PASS (all tests).

- [ ] **Step 5: Commit**

```bash
git add lib.ts lib.test.ts server.ts
git commit -m "Follow-up pills: validation helper and todos.suggestions column"
```

---

### Task 2: Tool, lifetime and API

**Files:**
- Modify: `apptools.ts:10` (import), `:17` (`WORKER_TOOLS`), `workerTools` (new tool after `set_pr`)
- Modify: `server.ts:540` (terminal `--allowedTools`), `summaries` (`:259`), the `messages` action (`:947` block)
- Modify: `runner.ts` `CONTRACT`, `commitMessage` (`:378`), `runWorker` (`:481`), `unprompted` (`:655`)

**Interfaces:**
- Consumes: `followUpError` (Task 1), `todos.suggestions` column.
- Produces: `summaries()` rows carry `suggestions: string[] | null`. `POST /api/todos/:id/messages` accepts `{ body, suggested?: true }`. The event kinds are `suggest` (actor `agent`) and `suggestion picked` (actor `user`).

- [ ] **Step 1: Add the tool** in `apptools.ts`

Import `followUpError` from `./lib` next to `lineCount`. In `WORKER_TOOLS`, append `"suggest_follow_ups"` to the name list. At the end of `workerTools`, after `set_pr`:

```ts
  // Suggested follow-ups (pills): the TODO's agent only, a terminal has no chat turn to follow up on.
  if (!g.via) s.registerTool("suggest_follow_ups", {
    description: "Suggest one or two follow-ups the user can send you with a click, each one line written as their instruction to you (e.g. \"Approve the PR\"). A later call replaces the earlier ones.",
    inputSchema: { items: z.array(z.string()) },
  }, async ({ items }) => {
    const e = followUpError(items);
    if (e) return err(e);
    const clean = items.map((x) => x.trim());
    setTodo(todoId, { suggestions: JSON.stringify(clean) }, false);
    addEvent(todoId, "suggest", "agent", clean.join(" · "));
    return ok();
  });
```

- [ ] **Step 2: Keep it away from terminals** in `server.ts:540`

Replace `"--allowedTools", WORKER_TOOLS.join(",")` with:

```ts
"--allowedTools", WORKER_TOOLS.filter((n) => !n.endsWith("__suggest_follow_ups")).join(",")
```

- [ ] **Step 3: Contract line** in `runner.ts` `CONTRACT`

Add `mcp__app__suggest_follow_ups` to the tool list in the first bullet (`…, mcp__app__set_worktree, mcp__app__set_pr and mcp__app__suggest_follow_ups.`). Then add this bullet after the "Clear your attention flag" bullet:

```
- When your turn ends with obvious next steps, call mcp__app__suggest_follow_ups with up to two of them, phrased as the instruction the user would send you (e.g. "Approve the PR"). When you asked the user a question, the suggestions can be its likely answers. The user sends one with a click.
```

- [ ] **Step 4: Clear the suggestions** in `runner.ts`

In `runWorker` and in `unprompted`, change `setTodo(todoId, { status: "running", last_tool: null });` to:

```ts
  setTodo(todoId, { status: "running", last_tool: null, suggestions: null });
```

In `commitMessage`, as the first line of the body:

```ts
  setTodo(todoId, { suggestions: null }, false);   // a sent message answers the suggestions, picked or not
```

- [ ] **Step 5: Hand them to the page** in `server.ts` `summaries`

Add `suggestions` to the destructuring (`({ file_paths, ctx, permission, config, overrides, suggestions, ...r })`) and to the returned object:

```ts
      suggestions: suggestions ? JSON.parse(suggestions) : null,
```

- [ ] **Step 6: Messages route** in `server.ts`, the `action === "messages"` branch

```ts
    } else if (action === "messages") {
      liveTodo(id);
      const text = String(body.body ?? "").trim();
      const uploads = uploadsIn(body.attachments);
      if (!text && !uploads.length) return fail("empty message");
      const mid = commitMessage(id, "user", text, uploads);
      // A pill click sends its text; the draft in the composer stays.
      if (body.suggested === true) addEvent(id, "suggestion picked", "user", text);
      else setDraft(`todo:${id}`, "");
      return json(messageOut(mid));
```

- [ ] **Step 7: Type-check and test**

Run: `bun install` (once; the worktree has no `node_modules`), then `bunx tsc --noEmit -p . 2>&1 | grep -E "apptools|runner|server|lib" ; bun test`
Expected: all tests pass. tsc may report errors that are already on `main`; only errors on lines this task touched count. Compare with the same tsc command run in `C:\code\switchboard`.

- [ ] **Step 8: Commit**

```bash
git add apptools.ts runner.ts server.ts
git commit -m "Follow-up pills: suggest_follow_ups tool, cleared at run start and on commit"
```

---

### Task 3: Pills in the chat and on the card

**Files:**
- Modify: `index.html`: CSS (next to `.composer` rules, ~line 495), `cardHtml` (~1855), `renderChat` (~2081), `ACTIONS` (~2600), the click dispatcher (~2683)

**Interfaces:**
- Consumes: `t.suggestions: string[] | null`, `t.status`, `t.permission` from `summaries()`. Also `POST /api/todos/:id/messages {body, suggested: true}`, and the page helpers `esc`, `call`, `toast`, `select`, `saveDraftSoon`, `drafts`, `draftKey`, `composer`, `cur`, `S`.
- Produces: `pillsHtml(t, where)`, where `where` is `"chat"` or `"card"`, and `ACTIONS.suggestion(id, el, e)`, where `id` is `"<todoId>:<index>"`.

- [ ] **Step 1: CSS** (after the `.composer-row textarea` rule)

```css
/* Suggested follow-ups (pills): the agent's next steps, a click sends one */
.pills { display: flex; flex-wrap: wrap; gap: 6px; padding: 8px 0 2px; }
.pill { max-width: 100%; padding: 3px 10px; border: 1px solid var(--line-cyan); border-radius: 999px; background: var(--tint-cyan); color: var(--accent-cyan); font: inherit; font-size: 12px; cursor: pointer; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.pill:hover { border-color: var(--accent-cyan); }
.card .pills { flex-wrap: nowrap; padding: 6px 0 0; }
.card .pill { flex: 0 1 auto; min-width: 0; }
```

- [ ] **Step 2: Render helper** (put it right before `function cardHtml`)

```js
// Suggested follow-ups (pills): shown while the agent waits for the user. The click reads the text from S.todos by index.
function pillsHtml(t, where) {
  if (!t?.suggestions?.length || t.status === "running" || t.permission) return "";
  const tip = where === "card" ? "Send to the agent · ⇧-click to open and edit first" : "Send to the agent · ⇧-click to edit first";
  return `<div class="pills">${t.suggestions.map((s, i) =>
    `<button type="button" class="pill" data-act="suggestion" data-id="${t.id}:${i}" title="${esc(s)}&#10;${tip}">↳ ${esc(s)}</button>`).join("")}</div>`;
}
```

`esc` (line 907) already escapes `& < > " '`, so it is safe in the `title` attribute.

- [ ] **Step 3: Card** in `cardHtml`, right after the `card-last` line:

```js
    ${pillsHtml(t, "card")}
```

- [ ] **Step 4: Chat** in `renderChat`. Replace the permission/running-line pair with:

```js
  if (!operator && running && t.permission) html += permHtml(t, true);
  else if (running) html += `<div class="running-line">…unchanged…</div>`;
  else if (!operator) html += pillsHtml(t, "chat");
```

Keep the existing running-line expression exactly as it is; only the third branch is new.

- [ ] **Step 5: Pass the event to actions** in the click dispatcher

```js
  ACTIONS[el.dataset.act]?.(el.dataset.id, el, e);
```

- [ ] **Step 6: The action** (add to `ACTIONS`)

```js
  // A pill: a click sends its text as the user's message; ⇧-click puts it in the composer to edit first.
  suggestion: async (id, el, e) => {
    const [tid, i] = id.split(":").map(Number);
    const t = (S.detail?.todo.id === tid && S.detail.todo) || S.todos.find((x) => x.id === tid);
    const text = t?.suggestions?.[i];
    if (!text) return;
    if (e?.shiftKey) {
      if (S.sel !== tid) select(tid);
      const key = `todo:${tid}`, d = drafts[key] ?? "";
      const next = d.trim() ? d.replace(/\s*$/, "\n") + text : text;
      saveDraftSoon(key, next);
      if (draftKey() === key) { composer.value = next; composer.focus(); }
      return render();
    }
    t.suggestions = null;   // gone at once; the server clears them too
    render();
    try { await call("POST", `/api/todos/${tid}/messages`, { body: text, suggested: true }); }
    catch (err) { toast("danger", "Not sent", err.message); }
  },
```

`saveDraftSoon` sets `drafts[key]` at once and saves to the server after 400 ms.

- [ ] **Step 7: Commit**

```bash
git add index.html
git commit -m "Follow-up pills: pills in the chat and on the board card"
```

---

### Task 4: Spec items and a run in the real app

**Files:**
- Modify: `designs/feature-spec-switchboard.md` (Chat section, after item 136; item 135's tool list)

- [ ] **Step 1: Spec item 137** (append after item 136):

```markdown
137. **Suggested follow-ups.** The TODO's agent can suggest one or two next steps with `suggest_follow_ups` (each one line, at most 80 characters, written as the user's instruction, e.g. "Approve the PR"); a later call in the same turn replaces them. They are kept in `todos.suggestions` and cleared when a run starts (also a background-task follow-up, item 136) and when any message is committed. While the TODO is not running and has no open permission request, they show as cyan pills under the last chat row and under the card's last message. A click sends the text as a normal `user` message (item 20) and records a `suggestion picked` event; the draft stays. ⇧-click puts the text into the composer (on the card: opens the TODO first). The agent's call is a `suggest` event. Claude terminals (item 135) do not get the tool, and the operator's chat has no pills.
```

In item 135, after the tool list `(`post_message`, …, `set_pr`)`, add: ` — not `suggest_follow_ups` (item 137)`.

- [ ] **Step 2: Unit tests**

Run: `bun test`
Expected: PASS.

- [ ] **Step 3: Run the app and check by hand** (use the `run` skill; `bun server.ts` on a spare `PORT`, with `SWITCHBOARD_HOME` set to a scratch folder so the real data is untouched)

Create a TODO in a test project with: `Say hello in chat, then suggest two follow-ups: "Say goodbye" and "Count to three".` Start it, and when the turn ends check:

1. Two pills show under the last chat row and on the board card.
2. Type a draft, then click "Say goodbye": the message is sent, the pills go away, and the draft is still in the composer.
3. The next turn (if the agent suggests nothing) shows no pills.
4. Ask for suggestions again. ⇧-click one on the board card: the TODO opens with the text in the composer, nothing is sent.
5. Reload the page while pills show: they are still there.
6. With Transcript on, the `suggest` and `suggestion picked` events show.

Take one screenshot of the chat with pills, save it to the TODO's artifact folder, and attach it.

- [ ] **Step 4: Commit**

```bash
git add designs/feature-spec-switchboard.md
git commit -m "Spec item 137: suggested follow-ups"
```

## Open Questions

| ID | Question | Options | Recommendation | Status |
|----|----------|---------|----------------|--------|

None.
