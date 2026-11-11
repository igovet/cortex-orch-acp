import { createSignal, Show, type JSX } from "solid-js";
import { Action, ActionBar, Page, Region, Row, SectionHead, StatusLane, Strip, Toolbar } from "../src/layout";
import { Modal } from "../src/kit";

// Every primitive at once, for the rendered layout gate. Real components and real sheets; only the words here are invented.
const TITLE_LONG = "Автоматическое изменение высоты полей ввода по мере ввода текста во всех формах создания задач, спринтов и сообщений, которое должно работать одинаково при любой длине названия и не ломать соседние элементы";
const BRANCH_LONG = "feature/an-extremely-long-branch-name-that-keeps-going-well-past-any-sensible-width-and-then-some-more-for-good-measure";
const REASON_LONG = `Вложения в формах создания задач, спринтов и во всех формах сообщений is stopped. Resume it first. «${TITLE_LONG}» waits for it, and so does everything queued after it in this sprint.`;
const UNBROKEN = "абвгдежзиклмн".repeat(9);
const SIZE = { width: "100%", height: "100%" };

export function Primitives(props: { stress: boolean; dialog: boolean; pane: number | null }) {
  const long = (normal: string, stressed: string) => props.stress ? stressed : normal;
  const [on, setOn] = createSignal(false);
  (window as unknown as { flip: () => void }).flip = () => setOn(v => !v);
  const why = (text: string) => on() ? undefined : text;
  const reason = long("Вложения в формах создания задач is stopped. Resume it first", REASON_LONG);
  const row = (i: number) => <Row lead={<span class="i i-eye s14" />} title={long(`installed fonts ship Cyrillic files ${i}`, `${TITLE_LONG} ${i}`)} detail={long("Runtime kept this claim unverified", `${UNBROKEN} ${REASON_LONG}`)} end={<span class="tnum">{i}m</span>} />;
  const card = (children: JSX.Element, startText: string) => <div style={{ border: "1px solid var(--line-strong)", "border-radius": "var(--r-card)", margin: "8px 12px", "background": "var(--bg-raised)" }}>
    <div style={{ padding: "9px 10px 7px", "font-size": "var(--t-sec)", "font-weight": "500", "overflow-wrap": "anywhere" }}>{long("Включить кнопку повторной генерации названия задачи", TITLE_LONG)}</div>
    <ActionBar place="card" start={<span class="cap faint">{startText}</span>}>{children}</ActionBar>
  </div>;
  return <div style={{ ...SIZE, width: props.pane ? `${props.pane}px` : "100%" }}>
    <Page scroll="none"
      head={<>
        <Toolbar label="Queue"
          start={<div class="seg" role="group" aria-label="Run mode"><button aria-current="false">Manual</button><button aria-current="true">Automatic</button></div>}
          mid={<a class="trunc" href="#next"><span class="faint">Next </span><b>{long("Автоматическое изменение высоты полей", TITLE_LONG)}</b></a>}
          end={<><button class="chip"><span class="faint l-compact-hide">Target branch</span><span class="mono l-branch">{long("dev", BRANCH_LONG)}</span></button><button class="ib" aria-label="Edit"><span class="i i-pencil s14" /></button>
            <Action kind="line" steady why={why(reason)} aria-describedby="queue-why">Continue queue</Action></>} />
        <Toolbar small start={<><button class="tab" aria-selected="true">Changes</button><button class="tab">Conversation</button></>} end={<button class="ib" aria-label="More"><span class="i i-more s14" /></button>} />
      </>}
      status={<>
        <Strip id="queue-why" tone="warn" action={<a class="lnk">Open task</a>}>{on() ? "Queue paused. Continue when you are ready." : `Queue paused: ${reason}`}</Strip>
        <StatusLane items={[{ id: "a", tone: "fail", text: long("One thing failed", REASON_LONG) }, { id: "b", tone: "info", text: "A note for later" }, { id: "c", tone: "ok", text: "All good here" }]} />
      </>}
      foot={<ActionBar start={<span class="cap">2 comments</span>} hint="Nothing left to send" label="Changes">
        <Action kind="line" why={why("Comments cannot be added without Git")}>Commit…</Action>
        <Action kind="primary" why={why(reason)}>{long("Merge", "Merge into dev")}</Action>
      </ActionBar>}>
      <div style={{ display: "grid", "grid-template-columns": "minmax(0, 1fr) minmax(0, 1fr)", height: "100%", "min-height": "0" }}>
        <Region>
          <SectionHead title={long("Check manually", TITLE_LONG)} count={11} />
          {row(1)}{row(2)}
          <div style={{ height: "16px" }} />
          <SectionHead title="Evidence" count={33} />
          {row(3)}
          <Show when={true}><Row line lead={<span class="i i-check s14" />} title={long("Files changed", BRANCH_LONG)} end="+120 −8" /></Show>
          {card(<><Action kind="primary" why={why("Waits for another task")}>Start</Action></>, "1h")}
          {card(<Action kind="line">Resume</Action>, "29m")}
          {row(4)}{row(5)}{row(6)}{row(7)}
        </Region>
        <Region>
          <SectionHead title="Several disabled in one bar" />
          <ActionBar start={<span class="cap">Result v1</span>}>
            <Action kind="line" why={why("No upstream branch")}>Push</Action>
            <Action kind="primary" why={why(reason)}>Accept v1</Action>
            <Action kind="icon" why={why("Not offered on this machine")} aria-label="More"><span class="i i-more s14" /></Action>
          </ActionBar>
          <SectionHead title="Helper text with nothing disabled" />
          <ActionBar hint={long("Accept moves the task to Ready to merge", REASON_LONG)}><Action kind="line">Request changes…</Action><Action kind="primary">Accept v1</Action></ActionBar>
          <SectionHead title="One reason, no left group" />
          <ActionBar><Action kind="primary" why={why(reason)}>Run sprint</Action></ActionBar>
          {row(8)}
        </Region>
      </div>
    </Page>
    <Modal open={props.dialog} onClose={() => undefined} label="Request changes">
      <div style={{ "--inset": "16px" }}><SectionHead title={long("Request changes", TITLE_LONG)} /></div>
      <div class="dlg-b"><label class="cap">What should change<textarea class="inp" style={{ display: "block", width: "100%", height: "88px" }} /></label></div>
      <ActionBar place="dialog" start={<button class="ib" aria-label="Attach"><span class="i i-plus s14" /></button>}>
        <Action kind="line">Cancel</Action>
        <Action kind="primary" why={why(long("Say what should change", REASON_LONG))}>Send and reopen</Action>
      </ActionBar>
    </Modal>
  </div>;
}
