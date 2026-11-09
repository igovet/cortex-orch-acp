// Opt-in fake-data render of the worker and conversation feed: static production bundle, no watchers or engines.
import {build} from "vite";
import {createServer} from "node:http";
import {spawn} from "node:child_process";
import {readFile, mkdir} from "node:fs/promises";
import {fileURLToPath} from "node:url";
const root=fileURLToPath(new URL("../../../",import.meta.url));
const out=root+".workspace-build/worker-feed-ui";
const label=process.argv[2]??"feed";
if(!/^[a-z0-9-]+$/.test(label))throw Error("Use a simple evidence label");
await mkdir(out+"/shots",{recursive:true});
await build({configFile:root+"packages/workspace-ui/vite.config.ts",root:root+"packages/workspace-ui",logLevel:"error",build:{outDir:out+"/bundle",emptyOutDir:true,rollupOptions:{input:root+"packages/workspace-ui/test/feed-visual.html"}}});
const server=createServer(async(req,res)=>{
 try {
  const path=new URL(req.url,"http://localhost").pathname;
  if(path.includes(".."))throw Error("Invalid path");
  const body=await readFile(out+"/bundle"+(path==="/"?"/test/feed-visual.html":path));
  res.setHeader("Content-Type",path.endsWith(".js")?"text/javascript":path.endsWith(".css")?"text/css":"text/html");
  res.end(body);
 } catch {res.writeHead(404);res.end();}
});
try {
 await new Promise(resolve=>server.listen(0,"127.0.0.1",resolve));
 const url="http://127.0.0.1:"+server.address().port+"/";
 const child=spawn("timeout",["--kill-after=5","170","docker","run","--rm","-i","--network","host","-v",out+"/shots:/evidence","cortex-workspace-rendered:ubuntu26.04","python3","-B","-",url,label],{stdio:["pipe","inherit","inherit"]});
 child.stdin.end(await readFile(new URL("./feed-visual.py",import.meta.url)));
 process.exitCode=await new Promise((resolve,reject)=>{child.on("error",reject);child.on("exit",code=>resolve(code??1));});
}finally{await new Promise(resolve=>server.close(resolve));}
