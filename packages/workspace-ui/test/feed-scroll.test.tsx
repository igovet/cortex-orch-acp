import { createRoot, createSignal } from "solid-js";
import { afterEach, expect, it, vi } from "vitest";
import { createFollow } from "../src/kit";
import { flush } from "./support";

let dispose: (()=>void)|undefined;
afterEach(()=>{dispose?.(); document.body.innerHTML=""; vi.unstubAllGlobals();});
it("pins streaming growth, preserves the reading anchor, and never writes during a scrollbar drag",async()=>{
  const box=document.createElement("div"), inner=document.createElement("div"), row=document.createElement("div"); inner.append(row);box.append(inner);document.body.append(box);
  let height=1000, top=600, rowY=0, resize!: ()=>void;
  Object.defineProperties(box,{clientHeight:{get:()=>400},scrollHeight:{get:()=>height},scrollTop:{get:()=>top,set:(v:number)=>{top=v;}}});
  box.getBoundingClientRect=()=>({top:0,bottom:400}) as DOMRect;
  row.getBoundingClientRect=()=>({top:rowY,bottom:rowY+100}) as DOMRect;
  vi.stubGlobal("ResizeObserver",class {constructor(cb:()=>void){resize=cb;}observe(){}disconnect(){}});
  const [n,setN]=createSignal(1);
  const follow=createRoot(d=>{dispose=d;return createFollow(()=>box,n);});
  await flush();
  height=1100; resize();
  expect(top).toBe(700);
  top=200;follow.onScroll();expect(follow.following()).toBe(false);
  height=1200;setN(2);resize();expect(top).toBe(200);
  rowY=50;resize();expect(top).toBe(250);
  box.dispatchEvent(new MouseEvent("pointerdown",{bubbles:true}));
  height=1300;rowY=150;resize();expect(top).toBe(250);
  top=900;follow.onScroll();document.dispatchEvent(new MouseEvent("pointerup",{bubbles:true}));
  expect(follow.following()).toBe(true);
  height=1400;resize();expect(top).toBe(1000);
});
