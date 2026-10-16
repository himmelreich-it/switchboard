// CodeMirror 6 for every file the app opens (item 107), as one global for index.html. Colors come from the
// design tokens, so Auto, Dark and Light need no extra code.
import { EditorState, Compartment, Annotation, Transaction, StateEffect, StateField, type Extension } from "@codemirror/state";
import { EditorView, Decoration, type DecorationSet, keymap, lineNumbers, drawSelection, highlightActiveLine, highlightActiveLineGutter, highlightSpecialChars } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { searchKeymap, highlightSelectionMatches, openSearchPanel } from "@codemirror/search";
import { syntaxHighlighting, HighlightStyle, StreamLanguage, bracketMatching, indentOnInput } from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import { markdown } from "@codemirror/lang-markdown";
import { javascript } from "@codemirror/lang-javascript";
import { json } from "@codemirror/lang-json";
import { css } from "@codemirror/lang-css";
import { html } from "@codemirror/lang-html";
import { python } from "@codemirror/lang-python";
import { yaml } from "@codemirror/lang-yaml";
import { shell } from "@codemirror/legacy-modes/mode/shell";
import { toml } from "@codemirror/legacy-modes/mode/toml";
import { go } from "@codemirror/legacy-modes/mode/go";
import { rust } from "@codemirror/legacy-modes/mode/rust";
import { dockerFile } from "@codemirror/legacy-modes/mode/dockerfile";
import { sql } from "@codemirror/legacy-modes/mode/sql";
import { lineDiff } from "./linediff";

// ponytail: no HCL/Terraform mode exists for CodeMirror 6; .tf files show as plain text.
function language(name: string): Extension {
  const n = name.toLowerCase(), ext = n.includes(".") ? n.slice(n.lastIndexOf(".") + 1) : "";
  if (ext === "md" || ext === "markdown") return markdown();
  if (["js", "mjs", "cjs", "jsx"].includes(ext)) return javascript({ jsx: true });
  if (["ts", "mts", "cts", "tsx"].includes(ext)) return javascript({ typescript: true, jsx: ext === "tsx" });
  if (ext === "json" || ext === "jsonc") return json();
  if (["css", "scss"].includes(ext)) return css();
  if (["html", "htm", "svg", "xml"].includes(ext)) return html();
  if (ext === "py") return python();
  if (ext === "yml" || ext === "yaml") return yaml();
  if (["sh", "bash", "zsh"].includes(ext) || n.startsWith(".env") || n === "makefile") return StreamLanguage.define(shell);
  if (ext === "toml") return StreamLanguage.define(toml);
  if (ext === "go") return StreamLanguage.define(go);
  if (ext === "rs") return StreamLanguage.define(rust);
  if (ext === "sql") return StreamLanguage.define(sql({}));
  if (n === "dockerfile" || n.endsWith(".dockerfile")) return StreamLanguage.define(dockerFile);
  return [];
}
const isMarkdown = (name: string) => /\.(md|markdown)$/i.test(name);

// Token colors from the v4 design: keyword violet, string amber, number magenta, comment muted, property cyan;
// markdown: heading cyan, quote violet, code amber, link lime.
const style = HighlightStyle.define([
  { tag: [t.keyword, t.controlKeyword, t.definitionKeyword, t.moduleKeyword, t.operatorKeyword, t.modifier], color: "var(--accent-violet)" },
  { tag: [t.string, t.special(t.string), t.regexp, t.character], color: "var(--accent-amber)" },
  { tag: [t.number, t.bool, t.null, t.atom], color: "var(--accent-magenta)" },
  { tag: [t.comment, t.lineComment, t.blockComment, t.meta], color: "var(--text-muted)" },
  { tag: [t.propertyName, t.attributeName, t.definition(t.propertyName)], color: "var(--accent-cyan)" },
  { tag: [t.heading, t.heading1, t.heading2, t.heading3], color: "var(--accent-cyan)", fontWeight: "600" },
  { tag: t.quote, color: "var(--accent-violet)" },
  { tag: t.monospace, color: "var(--accent-amber)" },
  { tag: [t.link, t.url], color: "var(--accent-lime)" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.strong, fontWeight: "700" },
  { tag: t.strikethrough, textDecoration: "line-through" },
  { tag: t.invalid, color: "var(--accent-red)" },
]);

const theme = EditorView.theme({
  "&": { height: "100%", color: "var(--text-primary)", backgroundColor: "var(--surface-inset)", fontSize: "13px" },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": { fontFamily: "var(--font-mono)", lineHeight: "20px" },
  ".cm-content": { padding: "12px 0", caretColor: "var(--accent-lime)" },
  ".cm-line": { padding: "0 32px 0 14px" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--accent-lime)", borderLeftWidth: "2px" },
  "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, ::selection": { backgroundColor: "var(--tint-lime)" },
  ".cm-gutters": { backgroundColor: "var(--surface-inset)", color: "var(--text-muted)", border: "none", borderRight: "1px solid var(--border-subtle)" },
  ".cm-lineNumbers .cm-gutterElement": { padding: "0 12px 0 16px" },
  ".cm-activeLine": { backgroundColor: "transparent" },
  "&.cm-focused .cm-activeLine": { backgroundColor: "var(--tint-neutral)" },
  ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--text-secondary)" },
  ".cm-selectionMatch": { backgroundColor: "var(--tint-cyan)" },
  ".sb-cmt": { backgroundColor: "var(--tint-amber)", borderBottom: "1px solid var(--line-amber)", cursor: "pointer" },
  ".sb-cmt.sb-cmt-on": { outline: "1px solid var(--accent-amber)" },
  ".cm-matchingBracket": { backgroundColor: "var(--tint-violet)", outline: "1px solid var(--line-violet)" },
  ".cm-searchMatch": { backgroundColor: "var(--tint-amber)", outline: "1px solid var(--line-amber)" },
  ".cm-searchMatch.cm-searchMatch-selected": { backgroundColor: "var(--tint-lime)" },
  ".cm-panels": { backgroundColor: "var(--surface-2)", color: "var(--text-primary)", fontFamily: "var(--font-mono)", fontSize: "12px" },
  ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--border-subtle)" },
  ".cm-panels.cm-panels-bottom": { borderTop: "1px solid var(--border-subtle)" },
  ".cm-textfield": { backgroundColor: "var(--surface-inset)", color: "var(--text-primary)", border: "1px solid var(--border-default)", borderRadius: "2px", fontFamily: "var(--font-mono)" },
  ".cm-button": { backgroundImage: "none", backgroundColor: "var(--surface-2)", color: "var(--text-primary)", border: "1px solid var(--border-strong)", borderRadius: "2px", fontFamily: "var(--font-mono)", textTransform: "uppercase", fontSize: "11px" },
  ".cm-panel.cm-search label": { color: "var(--text-secondary)" },
  ".cm-panel button[name=close]": { color: "var(--text-secondary)" },
  // markdown: wraps and reads like prose, line-height 1.65 as in the design's markdown editor
  "&.sb-md .cm-scroller": { lineHeight: "1.65" },
  "&.sb-md .cm-line": { padding: "0 20px" },
});

const external = Annotation.define<boolean>();

// Item 146: the comment highlights. The ranges move with edits; setMarks replaces them all.
type Mark = { from: number; to: number; id: number; active?: boolean };
const setMarksFx = StateEffect.define<Mark[]>();
const marksField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(set, tr) {
    set = set.map(tr.changes);
    for (const e of tr.effects) if (e.is(setMarksFx)) set = Decoration.set(e.value.filter((m) => m.to > m.from).map((m) =>
      Decoration.mark({ class: m.active ? "sb-cmt sb-cmt-on" : "sb-cmt", attributes: { "data-cid": String(m.id) } }).range(m.from, m.to)), true);
    return set;
  },
  provide: (f) => EditorView.decorations.from(f),
});

export type Handle = {
  view: EditorView;
  setDoc(text: string): void;
  setReadOnly(ro: boolean): void;
  focus(): void;
  search(): void;
  format(kind: string): void;
  setMarks(marks: Mark[]): void;
  reveal(pos: number): void;
  destroy(): void;
};

function create(parent: HTMLElement, o: { doc: string; name: string; readOnly?: boolean; onChange?: (text: string) => void }): Handle {
  const ro = new Compartment();
  const md = isMarkdown(o.name);
  const roExt = (r: boolean) => [EditorState.readOnly.of(r), EditorView.editable.of(!r)];
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc: o.doc,
      extensions: [
        md ? [] : [lineNumbers(), highlightActiveLineGutter()],
        highlightSpecialChars(), history(), drawSelection(), indentOnInput(), bracketMatching(), highlightActiveLine(), highlightSelectionMatches(),
        md ? EditorView.lineWrapping : [],
        md ? EditorView.editorAttributes.of({ class: "sb-md" }) : [],
        keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab]),
        language(o.name), syntaxHighlighting(style), theme,
        ro.of(roExt(!!o.readOnly)), marksField,
        EditorView.updateListener.of((u) => { if (u.docChanged && !u.transactions.some((tr) => tr.annotation(external))) o.onChange?.(u.state.doc.toString()); }),
      ],
    }),
  });
  let readOnly = !!o.readOnly;
  return {
    view,
    // Replaces the text without firing onChange: a clean file following the disk, or Discard.
    setDoc(text) {
      if (view.state.doc.toString() === text) return;
      const head = Math.min(view.state.selection.main.head, text.length);
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text }, selection: { anchor: head },
        annotations: [external.of(true), Transaction.addToHistory.of(false)] });
    },
    setReadOnly(r) { if (r !== readOnly) { readOnly = r; view.dispatch({ effects: ro.reconfigure(roExt(r)) }); } },
    focus() { view.focus(); },
    search() { openSearchPanel(view); },
    format(kind) { formatMd(view, kind); view.focus(); },
    setMarks(marks) { view.dispatch({ effects: setMarksFx.of(marks) }); },
    reveal(pos) { view.dispatch({ effects: EditorView.scrollIntoView(pos, { y: "center" }) }); },
    destroy() { view.destroy(); },
  };
}

// The markdown toolbar of the design (item 107): each button inserts markdown syntax around the selection or at the
// start of the selected lines.
function formatMd(view: EditorView, kind: string) {
  const s = view.state, r = s.selection.main, sel = s.sliceDoc(r.from, r.to);
  const wrap = (a: string, b = a, empty = "text") => {
    const body = sel || empty;
    view.dispatch({ changes: { from: r.from, to: r.to, insert: a + body + b }, selection: { anchor: r.from + a.length, head: r.from + a.length + body.length } });
  };
  const prefix = (p: (i: number) => string, toggle?: RegExp) => {
    const first = s.doc.lineAt(r.from).number, last = s.doc.lineAt(r.to).number;
    const changes = [];
    for (let n = first, i = 0; n <= last; n++, i++) {
      const line = s.doc.line(n), m = toggle && line.text.match(toggle);
      changes.push(m ? { from: line.from, to: line.from + m[0].length, insert: "" } : { from: line.from, insert: p(i) });
    }
    view.dispatch({ changes });
  };
  const block = (text: string) => {
    const line = s.doc.lineAt(r.to), before = line.text.trim() ? "\n\n" : "";
    view.dispatch({ changes: { from: line.to, insert: before + text + "\n" }, selection: { anchor: line.to + before.length + text.length + 1 } });
  };
  if (kind === "bold") wrap("**");
  else if (kind === "italic") wrap("_");
  else if (kind === "code") sel.includes("\n") ? wrap("```\n", "\n```") : wrap("`", "`", "code");
  else if (kind === "link") wrap("[", "](https://)", "link");
  else if (kind === "heading") prefix(() => "## ", /^#{1,6} /);
  else if (kind === "quote") prefix(() => "> ", /^> /);
  else if (kind === "ul") prefix(() => "- ", /^- (?!\[)/);
  else if (kind === "ol") prefix((i) => `${i + 1}. `, /^\d+\. /);
  else if (kind === "task") prefix(() => "- [ ] ", /^- \[[ x]\] /);
  else if (kind === "hr") block("---");
  else if (kind === "table") block("| Column | Column |\n| --- | --- |\n| | |");
}

Object.assign(globalThis, { SBEditor: { create, lineDiff } });
