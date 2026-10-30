# Switchboard

The UI follows the Claude Design versions in `designs/` (latest: `designs/Switchboard v10.html`). `designs/feature-spec-switchboard.md` describes the behaviour.

## UI changelog

An entry in `UI-CHANGELOG.md` asks Claude Design to look at a change. Add one, in the same commit, when a change to `index.html` or `electron/` makes the UI differ from the design: a new button, field, toast or click target; a moved label or panel; new UI text; a new layout or a chosen new width. Fixes count when they change the design this way.

No entry for a fix that only makes the UI render as the design already intends. Examples: table columns that wrapped one letter per line, text that overflowed its box, a button that did not react to a click. Claude Design has nothing to decide there.

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
