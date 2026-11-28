import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { engineSetupNotices } from "./engine-setup";

export function EngineSetupNotice(props: { read: () => Promise<{ engines?: Record<string, string> }> }) {
  const [notices, setNotices] = createSignal<ReturnType<typeof engineSetupNotices>>([]);
  onMount(() => {
    let active = true, timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const status = await props.read();
        if (active) setNotices(engineSetupNotices(status.engines));
      } catch { if (active) setNotices([]); }
      if (active) timer = setTimeout(() => void poll(), 3000);
    };
    void poll();
    onCleanup(() => { active = false; clearTimeout(timer); });
  });
  return <Show when={notices().length}><div class="strip info" role="status"><For each={notices()}>{notice =>
    <span>{notice.engines}: {notice.text} </span>
  }</For></div></Show>;
}
