// @vitest-environment jsdom
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { expect, it } from "vitest";
import { WsContext, type Ws } from "../src/ctx";
import { createSettingsState, useDraft } from "../src/settings-state";
import { SaveBar } from "../src/settings-kit";
import { flush } from "./support";

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
