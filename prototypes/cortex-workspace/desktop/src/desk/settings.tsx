import { For, Match, Show, Switch } from "solid-js";
import { FIXTURE_VERSION } from "../fixtures";
import { useDesk } from "./ctx";
import { MachinePanel } from "./panels";
import { TEXT_SIZES, type MotionPref, type TextSize, type ThemePref } from "./prefs";
import type { SettingsSection } from "./router";
import { Group, Row } from "./ui";

function Segmented<T extends string | number>(props: { label: string; value: T; options: [T, string][]; onChange: (v: T) => void; name: string }) {
  let group!: HTMLDivElement;
  const onKey = (e: KeyboardEvent) => {
    const i = props.options.findIndex(([v]) => v === props.value);
    const d = e.key === "ArrowRight" || e.key === "ArrowDown" ? 1 : e.key === "ArrowLeft" || e.key === "ArrowUp" ? -1 : 0;
    if (!d) return;
    e.preventDefault();
    const next = props.options[(i + d + props.options.length) % props.options.length][0];
    props.onChange(next);
    queueMicrotask(() => group.querySelector<HTMLElement>("[aria-checked=true]")?.focus());
  };
  return <div class="segmented" role="radiogroup" aria-label={props.label} ref={group} onKeyDown={onKey}>
    <For each={props.options}>{([v, l]) => <button type="button" role="radio" aria-checked={props.value === v} tabindex={props.value === v ? 0 : -1}
      class="seg" onClick={() => props.onChange(v)}>{l}</button>}</For>
  </div>;
}

export function SettingsPage(props: { section: SettingsSection; slug?: string }) {
  const desk = useDesk();
  const ps = () => props.slug ? desk.data.projectBySlug().get(props.slug) : undefined;
  return <div class="scroll-pane"><div class="page settings-col">
    <Switch>
      <Match when={props.section === "appearance"}>
        <h1 id="view-title" tabindex="-1">Appearance</h1>
        <Group label="Theme" id="set-theme">
          <Row label="Theme" desc={`System follows your desktop setting (currently ${desk.theme.systemDark() ? "dark" : "light"}).`}>
            <Segmented name="theme" label="Theme" value={desk.theme.pref()} onChange={v => desk.theme.setPref(v as ThemePref)}
              options={[["system", "System"], ["light", "Light"], ["dark", "Dark"]]} />
          </Row>
        </Group>
        <Group label="Text" id="set-text">
          <Row label="Interface text size" desc="Scales the whole interface. Layouts reflow; nothing auto-collapses.">
            <Segmented name="ui-size" label="Interface text size" value={desk.prefs.textSize()} onChange={v => desk.prefs.setTextSize(v as TextSize)}
              options={TEXT_SIZES.map(s => [s, `${s}%`] as [TextSize, string])} />
          </Row>
          <Row label="Content size" desc="Discussion and decision text only.">
            <Segmented name="content-size" label="Content size" value={desk.prefs.contentSize()} onChange={v => desk.prefs.setContentSize(v as TextSize)}
              options={TEXT_SIZES.map(s => [s, `${s}%`] as [TextSize, string])} />
          </Row>
        </Group>
        <Group label="Motion" id="set-motion">
          <Row label="Reduce motion" desc="System follows your desktop setting. On removes all animation.">
            <Segmented name="motion" label="Reduce motion" value={desk.prefs.motion()} onChange={v => desk.prefs.setMotion(v as MotionPref)}
              options={[["system", "System"], ["on", "On"], ["off", "Off"]]} />
          </Row>
        </Group>
      </Match>
      <Match when={props.section === "keyboard"}>
        <h1 id="view-title" tabindex="-1">Keyboard</h1>
        <Group label="Shortcuts" id="set-keys">
          <Row label="Single-key shortcuts" desc="J, K, H and L move between board cards while the board has focus. Turn off if they conflict with assistive technology.">
            <Segmented name="single" label="Single-key shortcuts" value={desk.prefs.singleKeys() ? "on" : "off"} onChange={v => desk.prefs.setSingleKeys(v === "on")}
              options={[["on", "On"], ["off", "Off"]]} />
          </Row>
          <Row label="All shortcuts" desc="Global shortcuts were not verified in a headed browser on the target desktop; some may be taken by the browser.">
            <button type="button" class="btn btn-secondary" onClick={() => desk.openHelp()}><span>Show shortcuts</span></button>
          </Row>
        </Group>
      </Match>
      <Match when={props.section === "about"}>
        <h1 id="view-title" tabindex="-1">About this mock</h1>
        <Group label="Prototype" id="set-about">
          <Row label="Data" desc="All actions change only in-memory state. Fixture data, not runtime evidence.">{FIXTURE_VERSION}</Row>
          <Row label="Connections" desc="No ACP, service, network, filesystem, Tauri or Git integration exists in this prototype.">None</Row>
          <Row label="Status" desc="Candidate direction pending the owner's Gate D review. Nothing here is approved or qualified.">Candidate</Row>
          <Row label="Round one" desc="Earlier explorations are kept for reference only.">
            <span class="btn-row"><a class="inline-link" href="/archive/b">Archived B</a><a class="inline-link" href="/archive/c">Archived C</a></span>
          </Row>
        </Group>
      </Match>
      <Match when={props.section === "project"}>
        <Show when={ps()} fallback={<><h1 id="view-title" tabindex="-1">Project not found</h1><p class="empty-row">No project matches this address.</p></>}>{p => <>
          <h1 id="view-title" tabindex="-1">{p().project.name}</h1>
          <p class="subline mono">{p().project.repository}</p>
          <MachinePanel ps={p()} />
        </>}</Show>
      </Match>
    </Switch>
  </div></div>;
}
