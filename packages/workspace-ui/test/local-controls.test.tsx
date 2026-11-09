// @vitest-environment jsdom
import { render } from "solid-js/web";
import { afterEach, expect, it, vi } from "vitest";
import { emptyWorkspace, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; localStorage.clear(); sessionStorage.clear(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const base = () => {
  const s = withTask(emptyWorkspace(), "Русская задача");
  s.machines[0].connection = "connected";
  s.repos.push({ projectId: "p", remote: "", tree: [], sources: {}, commits: [], branches: [{name:"main",where:"",task:null,ahead:0,behind:0}], fetchedAt: null,
    checkouts: [{ id:"p:main",projectId:"p",kind:"main",branch:"main",path:"/p",taskId:"t",label:null,base:"main",upstream:"",ahead:0,behind:0,behindOf:{},files:[],draft:"",sources:{},updateConflict:null }] });
  return s;
};
async function open(s: WorkspaceState, route: string, capabilities = {everything:true,operations:[] as string[]}) {
  const p = scriptedPort({capabilities});
  history.replaceState(null,"",route);
  const root=document.createElement("div"); document.body.append(root);
  dispose=render(()=><Workspace port={p.port}/>,root);
  await settle(); p.emit(projection("1",s)); await settle();
  return {root,p};
}
const button = (root: ParentNode, text: string) => [...root.querySelectorAll<HTMLButtonElement>("button")].find(b=>b.textContent?.trim()===text)!;

for (const kind of ["main","worktree"] as const) it(`task menu opens the actual ${kind} checkout`,async()=>{
  const s=base();
  if(kind==="worktree") {
    s.tasks[0].checkout={kind,branch:"task",path:"/trees/task"};
    s.repos[0].checkouts[0].taskId=null;
    s.repos[0].checkouts.push({...s.repos[0].checkouts[0],id:"p:tree",taskId:"t",kind,branch:"task",path:"/trees/task"});
  }
  const {root,p}=await open(s,"/m/local/t/t");
  root.querySelector<HTMLButtonElement>('[aria-label="Task actions"]')!.click(); await settle();
  const editor=[...root.querySelectorAll<HTMLButtonElement>("#m-task button")].find(b=>b.textContent?.startsWith("Open in VS Code"))!;
  expect(editor.getAttribute("aria-disabled")).not.toBe("true");
  editor.click(); await settle();
  expect(p.commands.at(-1)?.payload).toMatchObject({type:"open_editor",projectId:"p",checkoutId:kind==="main"?"p:main":"p:tree"});
});

it("an unavailable editor has a visible reason in the menu",async()=>{
  const {root}=await open(base(),"/m/local/t/t",{everything:false,operations:["tasks.start"]});
  root.querySelector<HTMLButtonElement>('[aria-label="Task actions"]')!.click(); await settle();
  const editor=[...root.querySelectorAll<HTMLButtonElement>("#m-task button")].find(b=>b.textContent?.startsWith("Open in VS Code"))!;
  expect(editor.getAttribute("aria-disabled")).toBe("true");
  expect(editor.textContent).toMatch(/editor|native|desktop|offered|available/i);
  expect(editor.textContent).not.toBe("Open in VS Code");
});

it("diagnostics show the failing check, recovery, and copy both",async()=>{
  const s=base();
  s.machines[0].diagnostics={at:1,status:"degraded",checks:8,items:[{name:"npm",ok:false,detail:"Not found",action:"Make npm available to the desktop service, then run diagnostics again."}]} as never;
  const writeText=vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator,"clipboard",{configurable:true,value:{writeText}});
  const {root}=await open(s,"/settings/machine/local/diagnostics");
  expect(root.textContent).toContain("npm");
  expect(root.textContent).toContain("Not found");
  expect(root.textContent).toContain("Make npm available");
  button(root,"Copy report").click();
  expect(writeText.mock.calls[0][0]).toContain("npm");
  expect(writeText.mock.calls[0][0]).toContain("Make npm available");
});

it("local-only Git has no dead network controls or push wording",async()=>{
  const {root}=await open(base(),"/m/local/p/p/git");
  expect(root.querySelector('button[aria-label="Push"],button[aria-label="Pull"],button[aria-label="Fetch"]')).toBeNull();
  expect(root.textContent).not.toMatch(/push|upstream/i);
});

for (const same of [false,true]) it(`local-only finish defaults are English (same=${same})`,async()=>{
  const s=base(),t=s.tasks[0];
  if(!same) { t.checkout.branch="task"; s.repos[0].checkouts[0].branch="task"; }
  Object.assign(t,{status:"ready_to_merge",workspaceDigest:"d",result:{version:1,digest:"d",summary:"Готово, работа выполнена",checks:[{name:"Check",outcome:"passed"}]},acceptance:{version:1,digest:"d",open:{}}});
  if(same) t.integrationFacts={targetBranch:"main",alreadyInTarget:false,sameBranch:true,commitAvailable:true,pendingPaths:["a.txt"],pushStatus:"unknown"};
  const {root}=await open(s,"/m/local/t/t/changes");
  expect(root.textContent).not.toMatch(/push|pull request|Create PR/i);
  button(root,same?"Commit to main…":"Merge…").click(); await settle();
  const dialog=root.querySelector("dialog[open]")!;
  expect(dialog.textContent).not.toMatch(/push|pull request|Create PR/i);
  const message=dialog.querySelector<HTMLTextAreaElement>("textarea")!.value;
  expect(message).toMatch(/Complete task/);
  expect(message).not.toMatch(/[А-Яа-я]/);
});

it("a missing worktree never opens the main project instead",async()=>{
  const s=base(); s.tasks[0].checkout={kind:"worktree",branch:"lost",path:"/lost"}; s.repos[0].checkouts[0].taskId=null;
  const {root,p}=await open(s,"/m/local/t/t");
  root.querySelector<HTMLButtonElement>('[aria-label="Task actions"]')!.click(); await settle();
  const editor=[...root.querySelectorAll<HTMLButtonElement>("#m-task button")].find(b=>b.textContent?.startsWith("Open in VS Code"))!;
  expect(editor.textContent).toContain("no longer available");
  expect(editor.getAttribute("aria-disabled")).toBe("true"); editor.click(); await settle();
  expect(p.commands).toHaveLength(0);
});

it("Git Generate is English even with Russian filenames",async()=>{
  const s=base(); s.repos[0].checkouts[0].files=[{path:"страница.html",status:"A",stage:"staged",hunks:[],add:1,del:0}];
  const {root,p}=await open(s,"/m/local/p/p/git"); button(root,"Generate").click(); await settle();
  expect(p.commands.at(-1)?.payload).toMatchObject({type:"git_draft",message:"Update 1 file"});
});

it("PR title default is English while the original task remains Russian",async()=>{
  const s=base(),t=s.tasks[0]; s.repos[0].remote="origin"; s.machines[0].ghAvailable=true;
  Object.assign(t,{status:"ready_to_merge",checkout:{kind:"main",branch:"task"},workspaceDigest:"d",result:{version:1,digest:"d",summary:"Готово",checks:[{name:"Test",outcome:"passed"}]},acceptance:{version:1,digest:"d",open:{}}});
  const {root}=await open(s,"/m/local/t/t/changes"); button(root,"Create PR…").click(); await settle();
  expect(root.querySelector<HTMLInputElement>("dialog[open] input.inp")!.value).toBe("Complete task changes");
});

it("worktree save refuses visibly and keeps the draft until a successful retry",async()=>{
  const {root,p}=await open(base(),"/settings/machine/local/worktrees");
  const input=root.querySelector<HTMLInputElement>('[aria-label="Worktree folder"]')!;
  input.value="/mnt/Big_Storage/.cortex/worktrees"; input.dispatchEvent(new Event("input",{bubbles:true}));
  button(root,"Save Worktrees on This machine").click(); await settle(); p.receipt(false); await settle();
  expect(root.querySelector('[role="alert"]')?.textContent).toContain("Refused");
  expect(input.value).toBe("/mnt/Big_Storage/.cortex/worktrees");
});
