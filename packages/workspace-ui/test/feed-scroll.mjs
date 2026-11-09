// Static production transforms; disposable service and private WebKitGTK screen, no watch mode.
import {build} from "vite";
import {createServer} from "node:http";
import {createConnection} from "node:net";
import {spawn} from "node:child_process";
import {readFile,mkdir,writeFile} from "node:fs/promises";
import {fileURLToPath} from "node:url";
import {createHash} from "node:crypto";
import {capabilities} from "../../workspace-contract/index.js";
const root=fileURLToPath(new URL("../../../",import.meta.url));
const label=process.argv[2]??"after";
if(!/^(before|after|final)$/.test(label))throw Error("Use before, after or final");
const out=root+".workspace-build/feed-scroll-ui/"+label;
await mkdir(out,{recursive:true});
const sources={};
for(const path of ["src/store.ts","src/port/real.ts","src/follow.ts","src/session.tsx","src/tool-names.ts","src/workspace.css"])
 sources[path]=createHash("sha256").update(await readFile(root+"packages/workspace-ui/"+path)).digest("hex");
const originalFollow=`import {createEffect,createSignal} from "solid-js";
export function createFollow(el,size){const [following,setFollowing]=createSignal(true),[missed,setMissed]=createSignal(0);let seen=size();createEffect(()=>{const n=size();if(n>seen&&!following())setMissed(m=>m+n-seen);seen=n;});const jump=()=>{setFollowing(true);setMissed(0);const b=el();if(b)b.scrollTop=0;};return{following,missed,jump,set:on=>on?jump():setFollowing(false),onScroll(){const away=(el()?.scrollTop??0)<-24;if(away&&following())setFollowing(false);else if(!away&&!following()){setFollowing(true);setMissed(0);}}};}`;
const plugins=label==="before"?[{name:"retained-pre-fix-scroll",enforce:"pre",transform(code,id){
 if(id.endsWith("/src/follow.ts"))return originalFollow;
 if(id.endsWith("/src/session.tsx"))return code.replace('const entry = (id: string) => entries().get(id)!;','const entry = (id: string) => m().session.timeline.find(e => e.id === id)!;');
 if(id.endsWith("/src/workspace.css"))return code.replace(".timeline, .stream { display: block; overflow-anchor: none; }","");
 // Reproduce the prior startup publication order in the isolated bundle only.
 if(id.endsWith("/src/store.ts"))return code.replace('const snapshots = await port.query("project.snapshot", {});','const snapshots = await port.query("project.snapshot", {}); for(const update of snapshots) install(update); setReady(true);');
}}]:[];
await build({configFile:root+"packages/workspace-ui/vite.config.ts",root:root+"packages/workspace-ui",logLevel:"error",plugins,
 build:{outDir:out+"/bundle",emptyOutDir:true,rollupOptions:{input:root+"packages/workspace-ui/test/feed-scroll.html"}}});
const seed=spawn("python3",["-B",root+"packages/workspace-ui/test/feed-scroll-startup.py"],{stdio:["pipe","pipe","inherit"]});
let info, infoText="";
const seedReady=new Promise((resolve,reject)=>{
 seed.stdout.on("data",chunk=>{infoText+=chunk;if(infoText.includes("\n")){try{info=JSON.parse(infoText.split("\n")[0]);resolve();}catch(e){reject(e);}}});
 seed.on("exit",code=>{if(!info)reject(Error("Seed service exited: "+code));});
});
const seedExit=new Promise(resolve=>seed.on("exit",code=>resolve(code??1)));
const rpc=(method,params)=>new Promise((resolve,reject)=>{
 const socket=createConnection(info.socket),id="probe",timer=setTimeout(()=>{socket.destroy();reject(Error("RPC timeout"));},15000);let data=Buffer.alloc(0);
 socket.on("error",reject);socket.on("close",()=>clearTimeout(timer));socket.on("connect",()=>{
 const body=Buffer.from(JSON.stringify({jsonrpc:"2.0",id,method,params})),header=Buffer.alloc(4);header.writeUInt32BE(body.length);socket.write(Buffer.concat([header,body]));});
 socket.on("data",chunk=>{data=Buffer.concat([data,chunk]);if(data.length<4||data.length<4+data.readUInt32BE(0))return;const reply=JSON.parse(data.subarray(4,4+data.readUInt32BE(0)));socket.end();reply.error?reject(Error(reply.error.message)):resolve(reply.result);});
});
let server;
try{
 await seedReady;
 await writeFile(out+"/sources.json",JSON.stringify({sources,service:"target/debug/workspace-service",label},null,2));
 server=createServer(async(req,res)=>{
  try{
   if(req.url==="/fixture"){res.setHeader("Content-Type","application/json");res.end(JSON.stringify({projectId:info.projectId,taskCount:Object.keys(info.tasks).length}));return;}
   if(req.url==="/rpc"){
    let body="";for await(const chunk of req)body+=chunk;const {operation,message}=JSON.parse(body);let result;
    if(operation==="capabilities.get"){const p=await rpc("projects.status",{projectId:info.projectId});result=capabilities("fixture","0",p.capabilities.operations);}
    else result=await rpc(["service.capabilities","projects.list","projects.status","machine.call"].includes(operation)?operation:"workspace",message);
    res.setHeader("Content-Type","application/json");res.end(JSON.stringify({result}));return;
   }
   const path=new URL(req.url,"http://localhost").pathname;if(path.includes(".."))throw Error("Invalid path");
   res.setHeader("Content-Type",path.endsWith(".js")?"text/javascript":path.endsWith(".css")?"text/css":"text/html");
   res.end(await readFile(out+"/bundle"+(path==="/"?"/test/feed-scroll.html":path)));
  }catch(error){res.writeHead(500);res.end(JSON.stringify({error:String(error)}));}
 });
 await new Promise(resolve=>server.listen(0,"127.0.0.1",resolve));
 const url="http://127.0.0.1:"+server.address().port+"/";
 const child=spawn("timeout",["--kill-after=5","140","docker","run","--rm","-i","--network","host","-v",out+":/evidence","cortex-workspace-rendered:ubuntu26.04","python3","-B","-",url,label],{stdio:["pipe","inherit","inherit"]});
 child.stdin.end(await readFile(new URL("./feed-scroll.py",import.meta.url)));
 process.exitCode=await new Promise((resolve,reject)=>{child.on("error",reject);child.on("exit",code=>resolve(code??1));});
}finally{
 if(server)await new Promise(resolve=>server.close(resolve));
 seed.stdin.end("\n");const code=await seedExit;if(code!==0)process.exitCode=code;
}
