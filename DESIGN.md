# auto-coder — Design

A local web UI that runs local Claude sessions. The first feature is PR review: you paste a PR URL. The app then:

1. creates or reuses a worktree for the PR;
2. reviews the PR with your stored prompt;
3. shows the findings;
4. lets you chat about them;
5. offers to **fix** them (your PR) or **post** them as review comments (someone else's PR).

## Stack

| Part | Choice |
|---|---|
| Server | Bun, one process, listening on `127.0.0.1` only, no login |
| Claude | `@anthropic-ai/claude-agent-sdk`, which uses the local Claude Code login |
| Storage | `bun:sqlite`, file `~/.auto-coder/db.sqlite` |
| Frontend | one `index.html` in plain JavaScript, no build step; `marked` from a CDN for markdown |
| Live updates | Server-Sent Events (`EventSource`) |
| GitHub | `gh` CLI, using whichever account is active |

The server has no login, so it must never listen on anything other than `127.0.0.1`. Anyone who reaches the port can make Claude edit files and post to GitHub as you.

## Flow

```
paste PR URL
  │
  ├─ 1. Code parses the URL          github.com/<owner>/<repo>/pull/<n>  (a trailing /files or #... is ignored)
  ├─ 2. Code matches the project     for each project: `git -C <path> remote get-url origin` contains "<owner>/<repo>"
  ├─ 3. Code reads PR data           gh pr view <url> --json headRefName,author
  ├─ 4. Code checks for a worktree   git -C <project> worktree list --porcelain
  │                                  contains "branch refs/heads/<headRefName>"?
  │        yes ─► git -C <worktree> pull ─► review
  │        no, or any step 1–4 fails, or the pull fails
  │           ─► Sonnet setup agent (see below) ─► review
  │
  ├─ Review session (read-only, cwd = worktree) ─► findings JSON
  │
  └─ UI: summary + finding cards + chat
        ├─ your PR:  [Fix selected] ─► diff + editable commit message ─► [Commit] ─► [Push]
        └─ their PR: [Post selected ▾ COMMENT | REQUEST_CHANGES | APPROVE]
```

### Setup agent

The setup agent runs only when the code checks in steps 1–4 do not give a usable worktree.

- Model: `claude-sonnet-5-5`, with `cwd` set to the matched project folder.
- It is called with `settingSources: ["user"]`, so your user skills load.
- Input: the PR URL, the project list, and whatever the code steps found, as a hint.
- The agent runs `/git-workflow:worktree <headRefName>`. The skill decides the worktree folder from `git config worktree.root`, or `<repo>/../worktrees` when that is not set. It also runs `copy-local-settings.sh`, which links `.env*` and `CLAUDE.local.md`, copies `.claude/`, `.vscode/` and `.idea/`, and runs `uv sync` when the worktree has a `pyproject.toml`.
- After the skill, the agent runs `gh pr checkout <n>` inside the new worktree. For a PR from a fork, the skill's `git worktree add -b <branch>` creates an empty branch, because `<headRefName>` does not exist on `origin`. `gh pr checkout` loads the PR's real code and sets the upstream. It names the local branch `<headRefName>`, so the step 4 check still finds the worktree next time.
- The agent can recover when a step fails, which fixed code cannot.
- Output: JSON `{project, pr_number, worktree_path, is_mine}`. The code checks that `worktree_path` exists before the review starts.
- **Check first, before building the rest:** a 5-line smoke test that the SDK with `settingSources: ["user"]` loads the plugin skill `/git-workflow:worktree`. If it does not, the app starts `claude -p "/git-workflow:worktree <branch>" --output-format json` as a child process instead, because the CLI loads plugins.

A second session on the same PR finds the worktree in step 4. It needs no agent and costs no setup tokens.

### Mine or theirs

`gh pr view --json author` is compared with `gh api user`, the active gh account. The app never switches accounts. The UI shows "posting as <login>" next to the post button.

## Sessions

- Each paste of a URL creates a new review session. Several sessions per PR are allowed. Your prompt tells Claude to read the PR's existing comments.
- Several reviews can run at the same time.
- A new session starts even if the worktree has uncommitted changes. Your prompt can tell Claude how to deal with them.
- A worktree is deleted only with the "Remove worktree" button.

### Review session

- Model: the `model` setting, default `claude-opus-5-5`.
- Read-only tools: `allowedTools: ["Read","Grep","Glob","Bash(git diff:*)","Bash(git log:*)","Bash(gh pr view:*)","Bash(gh pr diff:*)","Bash(gh api:*)"]`. `gh api` is on the list because `gh pr view --json` has no field for inline code comments. Reading those needs `gh api repos/{o}/{r}/pulls/{n}/comments`.
- A `canUseTool` callback refuses any `gh api` command that contains `-X`, `--method`, `-f`, `-F` or `--input`, because those turn it into a write.
- Prompt: your stored global prompt, followed by an output instruction the app adds:

```json
{
  "summary": "markdown",
  "findings": [
    {"id": 1, "file": "src/auth.ts", "line": 88, "severity": "high",
     "body": "Token expiry uses `<` instead of `<=`.",
     "suggested_fix": "if (expiry <= now) {"}
  ]
}
```

- Checks on the output:
  - If the JSON does not parse, the app resumes the same session once with "your JSON failed: <error>, resend only the JSON".
  - If the second attempt also fails, the raw text becomes the summary and no finding cards are shown.
  - A finding that lacks `file`, `line`, `severity` or `body` is dropped.

### Chat

- Chat resumes the same session (`resume: sessionId`), so Claude keeps its knowledge of the code.
- A chat reply may include a new findings JSON block. If it does, the UI replaces the finding list with it. Example: you write "drop 3", and Claude returns the list without 3.
- The new list is merged into the old one by `id`. A finding that survives keeps its `selected` state and any body text you edited in the UI.

### Fix (your PR)

- "Fix selected" resumes the session with `permissionMode: "acceptEdits"`, `cwd` set to the worktree, and the checked findings.
- Claude may edit files and run Bash in the worktree: `allowedTools: ["Bash"]` (`acceptEdits` does not auto-approve Bash). Blocked with `disallowedTools: ["Bash(git push:*)", "Bash(gh:*)"]`. When a command matches both lists, the disallowed list wins.
- At the end, Claude writes a commit message. The UI shows `git diff` and the message in an editable box.
- "Commit" runs `git add -A && git commit -m <message>`, plain code with no Claude. `-am` would skip files the fix created.
- "Push" runs `git push`, plain code with no Claude. `gh pr checkout` in the setup agent has already set the upstream.

### Post (their PR)

- The checked findings become one GitHub review: `gh api repos/{o}/{r}/pulls/{n}/reviews`, with one inline comment per finding at `file:line`.
- A finding's `suggested_fix` becomes a ```` ```suggestion ```` block, so the author can commit it with one click.
- Each comment is editable before posting, in Write / Preview tabs (`<textarea>` plus `marked`).
- The event dropdown offers COMMENT (the default), REQUEST_CHANGES and APPROVE.
- A finding whose line is not part of the diff goes into the review body instead of an inline comment, because GitHub rejects it as an inline comment.
- After a successful post, those findings are marked `posted`.

## Storage (`~/.auto-coder/db.sqlite`)

| Table | Columns |
|---|---|
| `projects` | `id, name, path` |
| `settings` | `key, value` (keys `prompt` and `model`) |
| `reviews` | `id, pr_url, project_id, pr_number, is_mine, worktree_path, session_id, status, summary, created_at` |
| `findings` | `id, review_id, file, line, severity, body, suggested_fix, selected, posted` |
| `messages` | `id, review_id, role, content, created_at` (chat log and tool-step lines) |

## UI

```
+------------+--------------------------------+------------------+
| Reviews    | acme/api #42  (theirs)         | Chat             |
| + New URL  | Summary (markdown)             |                  |
|            | [x] HIGH auth.ts:88  ...  edit | claude: ...      |
| api #42 ●  | [ ] LOW  util.ts:12  ...  edit | you: drop 2      |
| web #7  ✓  |                                |                  |
|            | [Post selected ▾ COMMENT]      | [type...] [Send] |
| ⚙ Settings |                                |                  |
+------------+--------------------------------+------------------+
```

- ● marks a review that is running. ✓ marks one that is done.
- The chat pane streams live. Each tool call is one grey line (`Read src/auth.ts`, `Bash git diff main...`). Claude's text shows as normal.
- Settings holds the global prompt, the model, and the projects list (add, edit, remove).

## Not in v1

- A "Re-review" button. Pasting the URL again starts a new session instead.
- Per-repo or named prompts.
- Switching gh accounts.
- Automatic worktree cleanup.
