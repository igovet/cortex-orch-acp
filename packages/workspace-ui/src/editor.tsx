import { createEffect, createMemo, on, onCleanup, onMount } from "solid-js";
import { defaultKeymap, history, historyKeymap, indentWithTab, isolateHistory } from "@codemirror/commands";
import { bracketMatching, HighlightStyle, indentOnInput, LanguageDescription, LanguageSupport, StreamLanguage, syntaxHighlighting, type StreamParser } from "@codemirror/language";
import { search, searchKeymap } from "@codemirror/search";
import { Compartment, EditorState, RangeSetBuilder, StateEffect, StateField, Transaction, type Extension, type TransactionSpec } from "@codemirror/state";
import { Decoration, drawSelection, EditorView, keymap, lineNumbers, type DecorationSet } from "@codemirror/view";
import { tags } from "@lezer/highlight";
import { useWs } from "./ctx";
import type { Buffers, FileBuffer } from "./editor-buffers";

const legacy = <T,>(load: () => Promise<StreamParser<T>>) => async () => new LanguageSupport(StreamLanguage.define(await load()));
const language = (name: string, extensions: string[], load: () => Promise<LanguageSupport>, filename?: RegExp) => LanguageDescription.of({ name, extensions, filename, load });
/** Each grammar is its own chunk, so a window pays only for the languages it opens. */
const LANGUAGES = [
  language("JavaScript", ["js", "mjs", "cjs"], async () => (await import("@codemirror/lang-javascript")).javascript()),
  language("JSX", ["jsx"], async () => (await import("@codemirror/lang-javascript")).javascript({ jsx: true })),
  language("TypeScript", ["ts", "mts", "cts"], async () => (await import("@codemirror/lang-javascript")).javascript({ typescript: true })),
  language("TSX", ["tsx"], async () => (await import("@codemirror/lang-javascript")).javascript({ jsx: true, typescript: true })),
  language("Python", ["py", "pyi"], async () => (await import("@codemirror/lang-python")).python()),
  language("Rust", ["rs"], async () => (await import("@codemirror/lang-rust")).rust()),
  language("JSON", ["json"], async () => (await import("@codemirror/lang-json")).json()),
  language("Markdown", ["md", "markdown"], async () => (await import("@codemirror/lang-markdown")).markdown()),
  language("HTML", ["html", "htm"], async () => (await import("@codemirror/lang-html")).html()),
  language("CSS", ["css"], async () => (await import("@codemirror/lang-css")).css()),
  language("YAML", ["yaml", "yml"], async () => (await import("@codemirror/lang-yaml")).yaml()),
  language("SQL", ["sql"], async () => (await import("@codemirror/lang-sql")).sql()),
  language("XML", ["xml", "svg"], async () => (await import("@codemirror/lang-xml")).xml()),
  language("C++", ["c", "h", "cc", "cpp", "hpp"], async () => (await import("@codemirror/lang-cpp")).cpp()),
  language("Go", ["go"], async () => (await import("@codemirror/lang-go")).go()),
  language("Java", ["java"], async () => (await import("@codemirror/lang-java")).java()),
  language("TOML", ["toml"], legacy(async () => (await import("@codemirror/legacy-modes/mode/toml")).toml)),
  language("Shell", ["sh", "bash", "zsh"], legacy(async () => (await import("@codemirror/legacy-modes/mode/shell")).shell)),
  language("Dockerfile", [], legacy(async () => (await import("@codemirror/legacy-modes/mode/dockerfile")).dockerFile), /^Dockerfile(\..+)?$/),
];
async function languageFor(path: string): Promise<Extension> {
  const found = LanguageDescription.matchFilename(LANGUAGES, path.split("/").at(-1) ?? path);
  return found ? await found.load().catch(() => []) : [];
}

/** The app's own syntax colours, so both themes follow the same variables as the diff views. */
const highlighting = HighlightStyle.define([
  { tag: [tags.keyword, tags.modifier, tags.controlKeyword, tags.operatorKeyword, tags.definitionKeyword, tags.moduleKeyword], color: "var(--syn-k)" },
  { tag: [tags.string, tags.special(tags.string), tags.regexp, tags.character, tags.attributeValue], color: "var(--syn-s)" },
  { tag: [tags.function(tags.variableName), tags.function(tags.propertyName), tags.labelName, tags.macroName], color: "var(--syn-f)" },
  { tag: [tags.comment, tags.lineComment, tags.blockComment, tags.docComment, tags.meta], color: "var(--syn-c)" },
  { tag: [tags.number, tags.bool, tags.null, tags.atom, tags.literal], color: "var(--syn-n)" },
  { tag: [tags.typeName, tags.className, tags.namespace, tags.tagName, tags.attributeName, tags.heading], color: "var(--syn-t)" },
  { tag: tags.strong, fontWeight: "600" }, { tag: tags.emphasis, fontStyle: "italic" },
  { tag: tags.link, color: "var(--syn-f)", textDecoration: "underline" },
]);
/** Quiet by rule: no focus ring, no shadow, and nothing marks the active line. */
const appearance = EditorView.theme({
  "&": { height: "100%", color: "var(--fg)", backgroundColor: "var(--bg-canvas)", fontSize: "var(--t-sec)" },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": { fontFamily: "var(--mono)", lineHeight: "20px" },
  ".cm-content": { caretColor: "var(--fg)" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--fg)" },
  ".cm-selectionBackground, &.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, ::selection": { backgroundColor: "var(--bg-selected)" },
  ".cm-gutters": { color: "var(--fg-3)", backgroundColor: "var(--bg-canvas)", border: "none" },
  ".cm-matchingBracket, &.cm-focused .cm-matchingBracket": { backgroundColor: "var(--bg-hover)", color: "inherit", outline: "none" },
  ".cm-nonmatchingBracket, &.cm-focused .cm-nonmatchingBracket": { backgroundColor: "var(--danger-tint)", color: "inherit" },
  ".cm-searchMatch": { backgroundColor: "var(--warn-tint)", outline: "none" },
  ".cm-searchMatch.cm-searchMatch-selected": { backgroundColor: "var(--bg-selected)" },
  ".cm-chg": { backgroundColor: "var(--add-bg)" },
  ".cm-panels": { color: "var(--fg)", backgroundColor: "var(--bg-sidebar)" },
  ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--line)" },
  ".cm-panel.cm-search": { padding: "6px 8px", fontFamily: "var(--font)", fontSize: "var(--t-cap)" },
  ".cm-panel.cm-search label": { fontSize: "var(--t-cap)", color: "var(--fg-2)" },
  ".cm-textfield": { color: "var(--fg)", backgroundColor: "var(--bg-input)", border: "1px solid var(--line-ctl)", borderRadius: "var(--r-ctl)", fontSize: "var(--t-cap)" },
  ".cm-button": { color: "var(--fg)", backgroundImage: "none", backgroundColor: "transparent", border: "1px solid var(--line-ctl)", borderRadius: "var(--r-ctl)", fontSize: "var(--t-cap)" },
  ".cm-button:active": { backgroundImage: "none", backgroundColor: "var(--bg-hover)" },
});

const setChanged = StateEffect.define<number[]>();
function decorate(state: EditorState, lines: number[]): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>(), mark = Decoration.line({ class: "cm-chg" });
  for (const n of [...lines].sort((a, b) => a - b)) if (n >= 1 && n <= state.doc.lines) { const at = state.doc.line(n).from; builder.add(at, at, mark); }
  return builder.finish();
}
/** The lines this checkout changed, carried through the owner's typing until the next diff arrives. */
const changedLines = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(value, tr) {
    const next = value.map(tr.changes);
    const effect = tr.effects.find(e => e.is(setChanged));
    return effect ? decorate(tr.state, effect.value as number[]) : next;
  },
  provide: field => EditorView.decorations.from(field),
});

const locked = new Compartment();
const lockOf = (path: string, reason: boolean) => [EditorState.readOnly.of(reason), EditorView.editable.of(!reason), EditorView.contentAttributes.of({ "aria-label": `${reason ? "Read" : "Edit"} ${path}` })];

type Session = { state: EditorState; scroll?: ReturnType<EditorView["scrollSnapshot"]> };
/** Each open file's editor state, undo history included, kept while its tab is in the background. */
type Held = { sessions: Map<string, Session>; listen: Extension; shown: { key: string; view: EditorView; onSave: () => void } | null };
const held = new WeakMap<Buffers, Held>();
function heldBy(buffers: Buffers) {
  const have = held.get(buffers);
  if (have) return have;
  const made: Held = { sessions: new Map(), shown: null, listen: [] };
  // Retained states outlive a mounted component: never capture its current file or save callback.
  made.listen = EditorView.updateListener.of(update => {
    const shown = made.shown;
    if (!update.docChanged || !shown || shown.view !== update.view) return;
    const session = made.sessions.get(shown.key);
    if (session) session.state = update.state;
    buffers.edit(shown.key, update.state.doc.sliceString(0, undefined, buffers.find(shown.key)?.eol ?? "\n"));
  });
  held.set(buffers, made);
  // Replacing the document as one edit is what leaves the owner's previous text in undo.
  buffers.subscribe((key, text, undoable) => {
    const session = made.sessions.get(key);
    if (!session) return;
    const spec = (state: EditorState): TransactionSpec => ({ changes: { from: 0, to: state.doc.length, insert: text }, annotations: undoable ? [isolateHistory.of("full")] : [Transaction.addToHistory.of(false)] });
    if (made.shown?.key === key) made.shown.view.dispatch(spec(made.shown.view.state));
    else session.state = session.state.update(spec(session.state)).state;
  });
  return made;
}

/** The editable file. The text it holds belongs to the buffer store, so this view can come and go without losing any. */
export default function CodeEditor(props: { buffer: FileBuffer; locked: boolean; line?: number; changed: Set<number>; onSave: () => void }) {
  const ws = useWs();
  const buffers = ws.buffers, keep = heldBy(buffers), sessions = keep.sessions;
  let host!: HTMLDivElement, view: EditorView | undefined, current: string | null = null, opening = 0;
  // on() tracks dependencies, not value equality. Snapshot object churn must not reset the view.
  const keyOf = createMemo(() => props.buffer.key), lock = createMemo(() => props.locked), lineOf = createMemo(() => props.line);
  const changed = createMemo(() => props.changed, undefined, { equals: (a, b) => a.size === b.size && [...a].every(n => b.has(n)) });
  const remember = () => { if (view && current) { const session = sessions.get(current); if (session) { session.state = view.state; session.scroll = view.scrollSnapshot(); } } };
  const commands = keymap.of([{ key: "Mod-s", preventDefault: true, run: () => { keep.shown?.onSave(); return true; } }, ...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab]);

  async function stateFor(buffer: FileBuffer) {
    const have = sessions.get(buffer.key);
    if (have) return have;
    const grammar = await languageFor(buffer.path);
    const made = sessions.get(buffer.key) ?? { state: EditorState.create({ doc: buffers.text(buffer.key), extensions: [
      lineNumbers(), history(), drawSelection(), indentOnInput(), bracketMatching(), search({ top: true }), syntaxHighlighting(highlighting), appearance, changedLines, commands, keep.listen, grammar,
      locked.of(lockOf(buffer.path, props.locked)),
    ] }) };
    sessions.set(buffer.key, made);
    return made;
  }
  const markChanged = () => view?.dispatch({ effects: setChanged.of([...changed()]) });
  const goTo = () => {
    const line = props.line;
    if (!view || line === undefined || line < 1 || line > view.state.doc.lines) return;
    const at = view.state.doc.line(line).from;
    view.dispatch({ selection: { anchor: at }, effects: EditorView.scrollIntoView(at, { y: "center" }) });
  };

  onMount(() => { view = new EditorView({ parent: host }); });
  onCleanup(() => { ++opening; remember(); keep.shown = null; view?.destroy(); view = undefined; });
  createEffect(on(keyOf, key => {
    const turn = ++opening;
    remember();
    void stateFor(props.buffer).then(session => {
      if (turn !== opening || !view) return;
      current = key;
      keep.shown = { key, view, onSave: () => props.onSave() };
      view.setState(session.state);
      if (session.scroll) view.dispatch({ effects: session.scroll });
      view.dispatch({ effects: locked.reconfigure(lockOf(props.buffer.path, props.locked)) });
      markChanged();
      goTo();
    });
  }));
  createEffect(on(lock, off => { if (view && current) view.dispatch({ effects: locked.reconfigure(lockOf(props.buffer.path, off)) }); }, { defer: true }));
  createEffect(on(changed, () => { if (current) markChanged(); }, { defer: true }));
  createEffect(on(lineOf, () => { if (current) goTo(); }, { defer: true }));
  return <div class="ws-cm" ref={host} />;
}
