import { createSignal, Show } from "solid-js";
import { engineLabel, type EngineKey, type Task, type TaskSpend } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { createTicker } from "./freshness";
import { I } from "./kit";
import { stallHours } from "./notifications";
import { clock } from "./selectors";

const HOUR = 3_600_000;

/** "3 h 12 min", "45 min", "2 d 4 h". */
export function spendDuration(ms: number): string {
  const minutes = Math.floor(Math.max(0, ms) / 60_000), d = Math.floor(minutes / 1440), h = Math.floor(minutes / 60) % 24, m = minutes % 60;
  if (d === 0 && h === 0) return `${m} min`;
  if (d === 0) return m === 0 ? `${h} h` : `${h} h ${m} min`;
  return h === 0 ? `${d} d` : `${d} d ${h} h`;
}
/** "950", "34k", "1.2M". */
export function spendCount(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${Math.round(n / 1000)}k`;
  const tenths = Math.round(n / 100_000);
  return tenths % 10 === 0 ? `${tenths / 10}M` : `${(tenths / 10).toFixed(1)}M`;
}
const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;
/** Tokens as reported: "Not reported" when no turn reported any, and how many turns did when only some did. Never zero for unknown. */
function tokenText(tokens: number | null, reported: number, turns: number): string {
  if (tokens === null) return "Not reported";
  return reported < turns ? `${spendCount(tokens)}, reported for ${reported} of ${plural(turns, "turn")}` : spendCount(tokens);
}

/** The rows of a task's Spend section. A task with nothing recorded yet has none. */
export function spendRows(spend: TaskSpend | undefined): [string, string][] {
  if (!spend || (spend.workingMs === 0 && spend.sessions === 0 && spend.turns === 0)) return [];
  const rows: [string, string][] = [["Time working", spendDuration(spend.workingMs)], ["Engine sessions", String(spend.sessions)], ["Turns", String(spend.turns)],
    ["Tokens", tokenText(spend.tokens, spend.reportedTurns, spend.turns)]];
  for (const e of spend.engines) {
    const name = e.engine ? `${engineLabel(e.engine as EngineKey)} · ${e.modelLabel ?? e.model}` : "Unattributed";
    const tokens = e.tokens === null ? "tokens not reported" : `${tokenText(e.tokens, e.reportedTurns, e.turns)} tokens`;
    rows.push([name, `${plural(e.sessions, "session")}, ${plural(e.turns, "turn")}, ${tokens}`]);
  }
  if (spend.lastProgressAt !== null) rows.push(["Last progress", clock(spend.lastProgressAt)]);
  if (spend.lastFailures.length) rows.push(["Last attempts hit", spend.lastFailures.join("; ")]);
  return rows;
}

/** One line for the top of Work details: "3 h 12 min working · 2 sessions · 42 turns · 1.2M tokens". */
export function spendLine(spend: TaskSpend | undefined): string | null {
  if (!spend || (spend.workingMs === 0 && spend.sessions === 0 && spend.turns === 0)) return null;
  const tokens = spend.tokens === null ? "tokens not reported" : `${tokenText(spend.tokens, spend.reportedTurns, spend.turns)} tokens`;
  return [`${spendDuration(spend.workingMs)} working`, plural(spend.sessions, "session"), plural(spend.turns, "turn"), tokens].join(" · ");
}
export function SpendLine(props: { task: Task }) {
  return <Show when={spendLine(props.task.spend)}>{line => <div class="cap muted ws-spend" aria-label="Spend">Spent {line()}</div>}</Show>;
}

/** Whole hours a working task has gone without progress, once that reached the owner's threshold; null otherwise or when Off. */
export function stalledHours(task: Pick<Task, "status" | "spend">, now: number, threshold: number): number | null {
  const since = task.spend?.stalledSince;
  if (task.status !== "working" || since == null || threshold <= 0) return null;
  const hours = Math.floor((now - since) / HOUR);
  return now - since >= threshold * HOUR ? hours : null;
}

/** A quiet "No progress for N h" beside the task's actions while it stays stuck. Informational only: it changes nothing. */
export function StallBadge(props: { task: Task }) {
  const [hours] = stallHours();
  const ws = useWs();
  const [now, setNow] = createSignal(ws.store.clock());
  createTicker(() => setNow(ws.store.clock()), 60_000);
  const stuck = () => stalledHours(props.task, now(), hours());
  return <Show when={stuck() !== null}>
    <span class="chip ws-stall" role="status" aria-label={`No new result since ${clock(props.task.spend!.stalledSince!)}. The task keeps working.`}>
      <I n="hourglass" s={12} class="st-warn" />No progress for {stuck()} h</span>
  </Show>;
}
