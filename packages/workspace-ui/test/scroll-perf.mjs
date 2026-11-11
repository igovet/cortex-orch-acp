// The scroll gate: every scrolling surface of the static production bundle, on the layout gate's fake data made long, in
// WebKitGTK with compositing on as the desktop window runs it. Thresholds are in scroll-perf.py; a breach exits nonzero.
// Evidence (numbers and captures) goes to the git-ignored .design-evidence/conversation/scroll-perf/<label>.
import {build} from "vite";
import {createServer} from "node:http";
import {spawn} from "node:child_process";
import {readFile, mkdir} from "node:fs/promises";
import {fileURLToPath} from "node:url";
const root=fileURLToPath(new URL("../../../",import.meta.url));
const label=process.argv[2]??"scroll";
if(!/^[a-z0-9-]+$/.test(label))throw Error("Use a simple evidence label");
const out=root+".workspace-build/scroll-perf/"+label;
const shots=root+".design-evidence/conversation/scroll-perf/"+label;
await mkdir(shots,{recursive:true});
await build({configFile:root+"packages/workspace-ui/vite.config.ts",root:root+"packages/workspace-ui",logLevel:"error",build:{outDir:out+"/bundle",emptyOutDir:true,rollupOptions:{input:root+"packages/workspace-ui/test/layout-fit.html"}}});
const server=createServer(async(req,res)=>{
 try {
  const path=new URL(req.url,"http://localhost").pathname;
  if(path.includes(".."))throw Error("Invalid path");
  const body=await readFile(out+"/bundle"+(path.startsWith("/assets/")?path:"/test/layout-fit.html"));
  res.setHeader("Content-Type",path.endsWith(".js")?"text/javascript":path.endsWith(".css")?"text/css":"text/html");
  res.end(body);
 } catch {res.writeHead(404);res.end();}
});
try {
 await new Promise(resolve=>server.listen(0,"127.0.0.1",resolve));
 const url="http://127.0.0.1:"+server.address().port+"/";
 const child=spawn("timeout",["--kill-after=5","1500","docker","run","--rm","-i","--network","host","--user",process.getuid()+":"+process.getgid(),"-e","HOME=/tmp","-v",shots+":/evidence","cortex-workspace-rendered:ubuntu26.04","python3","-B","-u","-",url,label,...process.argv.slice(3)],{stdio:["pipe","inherit","inherit"]});
 child.stdin.end(await readFile(new URL("./scroll-perf.py",import.meta.url)));
 process.exitCode=await new Promise((resolve,reject)=>{child.on("error",reject);child.on("exit",code=>resolve(code??1));});
}finally{await new Promise(resolve=>server.close(resolve));}
