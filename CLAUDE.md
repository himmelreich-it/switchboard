# Switchboard

The UI follows the Claude Design versions in `designs/` (latest: `designs/Switchboard v9.html`). `designs/feature-spec-switchboard.md` describes the behaviour.

## UI changelog

Every change that alters what the UI looks like or how it reacts (`index.html`, `electron/`) gets an entry in `UI-CHANGELOG.md`, in the same commit. Fixes count too: a new button, a moved label, a changed width, a new toast, a new click target.

Add the entry at the top of the "Open" section:

```markdown
### 2026-10-15 · Open in VS Code button
- Where: Focus header › topic popover.
- Now: a button "Open in VS Code" opens the TODO's worktree, or the topic folder. Success shows the toast "Opened in VS Code · <folder>".
- Why: the user wanted to jump from a TODO into the editor.
- Design question: does the button belong in the popover, or in the header next to the worktree chip?
```

Write each entry so it can be pasted into Claude Design on its own: no "see commit", no file or function names in "Now". The heading carries no commit hash, because the entry goes into the commit it would name.

The entries are fed to Claude Design from time to time. When a new design version covers an entry, move the entry under that version's heading in "Covered".
