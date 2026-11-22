// @vitest-environment jsdom
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { expect, it } from "vitest";
import { WsContext, type Ws } from "../src/ctx";
import { createSettingsState, useDraft } from "../src/settings-state";
import { SaveBar } from "../src/settings-kit";
import { flush } from "./support";

it.each(["routing", "project_ceiling"])("submits only one %s save while its reply is pending", async surface => {
  const root = document.createElement("div"); document.body.append(root);
  const settings = createSettingsState();
  let draft!: ReturnType<typeof useDraft<string>>;
  let calls = 0;
  let release!: () => void;
  const reply = new Promise<void>(resolve => { release = resolve; });
  const dispose = render(() => {
    const [saved, setSaved] = createSignal("before");
    const [revision, setRevision] = createSignal(1);
    const ws = { settings, demo: null } as unknown as Ws;
    const Form = () => {
      draft = useDraft({ key: () => `local:project:${surface}`, saved, revision, name: () => surface, where: () => "Project",
        commit: async (value, expected) => {
          const requestId = String(++calls);
          if (expected !== revision()) return { requestId, ok: false, outcome: "revision_conflict", message: "nothing was written" };
          // The service has committed, but the client has not received the reply.
          setRevision(2);
          await reply;
          setSaved(value);
          return { requestId, ok: true, outcome: "recorded", message: "" };
        } });
      return <SaveBar d={draft} />;
    };
    return <WsContext.Provider value={ws}><Form /></WsContext.Provider>;
  }, root);
  try {
    draft.set("after");
    const first = draft.save();
    const duplicate = settings.saveCurrent();
    release();
    expect(await Promise.all([first, duplicate])).toEqual([true, true]);
    expect(calls).toBe(1);
    expect(draft.state()).toBeNull();
    expect(root.textContent).not.toContain("nothing was written");
  } finally { dispose(); root.remove(); }
});


it("structured conflicts keep a dirty draft and require review before reapplying at the refreshed revision",async()=>{
  const commits: { value: {language:string}; revision: number|undefined }[]=[];
  let draft!: ReturnType<typeof useDraft<{language:string}>>;
  let update!: () => void;
  const root=document.createElement("div"); document.body.append(root);
  const dispose=render(()=>{
    const [saved,setSaved]=createSignal({language:"auto"});
    const [revision,setRevision]=createSignal(1);
    update=()=>{setSaved({language:"ru"});setRevision(2);};
    const ws={settings:createSettingsState(),panels:{},demo:null} as unknown as Ws;
    const Form=()=>{
      draft=useDraft({key:()=>"local:user_defaults",saved,revision,name:()=>"Defaults",where:()=>"Desk",
        commit:async(value,rev)=>{
          commits.push({value,revision:rev});
          return {requestId:"r",ok:rev===revision(),outcome:rev===revision()?"recorded":"revision_conflict",message:"Localised refusal without magic English words"};
        }});
      return <SaveBar d={draft}/>;
    };
    return <WsContext.Provider value={ws}><Form/></WsContext.Provider>;
  },root);
  try {
    draft.set({language:"en"}); update();
    expect(await draft.save()).toBe(false);
    expect(draft.state()).toBe("conflict");
    expect(draft.value()).toEqual({language:"en"});
    expect(commits[0].revision).toBe(1);
    const button=(text:string)=>[...root.querySelectorAll<HTMLButtonElement>("button")].find(b=>b.textContent===text)!;
    expect(button("Reapply mine")).toBeUndefined();
    button("Review changes").click();
    expect(root.textContent).toContain("collision");
    button("Reapply mine").click(); await flush();
    expect(commits[1]).toEqual({value:{language:"en"},revision:2});
    expect(draft.state()).toBeNull();
  } finally {dispose();root.remove();}
});
