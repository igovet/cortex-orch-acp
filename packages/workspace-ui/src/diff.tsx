import { createMemo, createSignal, For, Show } from "solid-js";
import type { DiffHunk, DiffLine, FileDiff, ReviewComment } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { highlight } from "./highlight";
import { ResultText } from "./result-text";
import { partialDiff, splitRows, withoutSpace } from "./review-model";

export const Code = (props: { path: string; text: string }) =>
  <For each={highlight(props.path, props.text)}>{t => t.c ? <span class={t.c}>{t.t}</span> : t.t}</For>;

export const Counts = (props: { add: number; del: number }) => <span class="tnum"><Show when={props.add > 0 || props.del === 0}><span class="add">+{props.add}</span></Show>{props.add > 0 && props.del > 0 ? " " : ""}<Show when={props.del > 0}><span class="del">−{props.del}</span></Show></span>;
export const STATUS_WORD: Record<string, string> = { M: "Modified", A: "Added", D: "Deleted", U: "Untracked", C: "Conflict" };
export const GitLetter = (props: { status: string }) => <span class={`gs gs-${props.status.toLowerCase()}`}>{props.status}</span>;

function CommentForm(props: { value: string; onSave: (text: string) => void; onCancel: () => void }) {
  const [text, setText] = createSignal(props.value);
  return <div class="cmt"><textarea class="inp ws-cmt-in" rows="2" aria-label="Comment" value={text()} onInput={e => setText(e.currentTarget.value)} ref={el => queueMicrotask(() => el.focus())}
    onKeyDown={e => { if (e.key === "Escape") { e.stopPropagation(); props.onCancel(); } }} />
    <div class="cmt-f"><span class="grow"></span><button class="btn btn-sm" onClick={() => props.onCancel()}>Cancel</button><button class="btn btn-line btn-sm" aria-disabled={text().trim() ? undefined : "true"} data-why={text().trim() ? undefined : "Type a comment first"} onClick={() => { if (text().trim()) props.onSave(text()); }}>Save comment</button></div></div>;
}

function Comment(props: { taskId: string; c: ReviewComment }) {
  const ws = useWs();
  const [editing, setEditing] = createSignal(false);
  return <Show when={editing()} fallback={<div class="cmt"><div class="cmt-b"><ResultText breaks text={props.c.text} /></div>
    <div class="cmt-f">{props.c.sent ? "Sent" : "Your draft comment"} on line {props.c.line}<span class="grow"></span>
      <Show when={!props.c.sent}><button class="btn btn-sm" onClick={() => setEditing(true)}>Edit</button><button class="btn btn-sm" {...ws.needs("remove_comment")} onClick={() => ws.run({ type: "remove_comment", taskId: props.taskId, commentId: props.c.id })}>Delete</button></Show></div></div>}>
    <CommentForm value={props.c.text} onCancel={() => setEditing(false)} onSave={text => { ws.ok({ type: "edit_comment", taskId: props.taskId, commentId: props.c.id, text }, () => { setEditing(false); }); }} />
  </Show>;
}

export type HunkActions = { reason?: string; stage: (hunk: number) => void; revert: (hunk: number) => void; staged: (hunk: number) => boolean };
const lineOf = (l: DiffLine) => l.new ?? l.old!;

/** One file's diff. A line takes a comment from the plus beside its number; comments gather into one message. */
export function Diff(props: { file: FileDiff; layout: "unified" | "split"; hideSpace?: boolean; actions?: HunkActions; taskId?: string; comments?: ReviewComment[] }) {
  const ws = useWs();
  const [adding, setAdding] = createSignal<number | null>(null);
  const hunks = createMemo(() => props.hideSpace ? props.file.hunks.map(withoutSpace) : props.file.hunks);
  const at = (line: number) => (props.comments ?? []).filter(c => c.line === line);
  const head = (h: DiffHunk, i: number) => <div class="dl hunk"><span>{h.header}</span>
    <Show when={props.actions}>{a => <span class="hact">
      <button class="btn btn-sm" aria-disabled={a().reason ? "true" : undefined} data-why={a().reason} aria-pressed={a().staged(i)} onClick={() => { if (!a().reason) a().stage(i); }}>{a().staged(i) ? "Unstage hunk" : "Stage hunk"}</button>
      <button class="btn btn-sm" aria-disabled={a().reason ? "true" : undefined} data-why={a().reason} onClick={() => { if (!a().reason) a().revert(i); }}>Revert hunk</button></span>}</Show></div>;
  const save = (text: string, line: number) => { if (props.taskId) ws.ok({ type: "add_comment", taskId: props.taskId, path: props.file.path, line, text }, () => { setAdding(null); }); };
  const below = (l: DiffLine) => <Show when={props.taskId}>
    <For each={at(lineOf(l))}>{c => <Comment taskId={props.taskId!} c={c} />}</For>
    <Show when={adding() === lineOf(l)}><CommentForm value="" onCancel={() => setAdding(null)} onSave={text => save(text, lineOf(l))} /></Show>
  </Show>;
  const mark = (l: DiffLine) => l.kind === "add" ? "+" : l.kind === "del" ? "−" : "";
  return <>
    <Show when={props.file.hunks.length > 0} fallback={<div class="cap muted ws-pad">{ws.t("noDiff")}</div>}>
      <Show when={props.layout === "unified"} fallback={<div class="sbs">
        <For each={[0, 1]}>{side => <div class="side"><For each={hunks()}>{h => <>
          <div class="sl gap"><span class="n"></span><span class="m"></span><span class="c faint">{side === 0 ? h.header : ""}</span></div>
          <For each={splitRows(h)}>{row => {
            const line = () => side === 0 ? row.left : row.right;
            return <Show when={line()} fallback={<div class="sl gap"><span class="n"></span><span class="m"></span><span class="c"></span></div>}>{l =>
              <div class={`sl${l().kind === "ctx" ? "" : ` ${l().kind}`}`}><span class="n">{side === 0 ? l().old : l().new}</span><span class="m">{mark(l())}</span><span class="c"><Code path={props.file.path} text={l().text} /></span></div>}</Show>;
          }}</For>
        </>}</For></div>}</For>
      </div>}>
        <div class="code"><For each={hunks()}>{(h, i) => <>
          {head(h, i())}
          <For each={h.lines}>{l => <>
            <div class={`dl${l.kind === "ctx" ? "" : ` ${l.kind}`}`}><span class="n">{l.old}</span><span class="n">{l.new}</span>
              <span class="m ws-m">{mark(l)}<Show when={props.taskId}><button class="ws-plus" aria-label={`Comment on line ${lineOf(l)}`} onClick={() => setAdding(lineOf(l))}>+</button></Show></span>
              <span class="c"><Code path={props.file.path} text={l.text} /></span></div>
            {below(l)}
          </>}</For>
        </>}</For></div>
      </Show>
    </Show>
    <Show when={partialDiff(props.file) && props.file.hunks.length > 0}><div class="cap faint ws-pad">{ws.t("partialDiff")}</div></Show>
  </>;
}
