// Opt-in no-paid CPU diagnostic. Own window, production transforms, no engine/store.
import {build} from "vite";
import {createServer} from "node:http";
import {spawn} from "node:child_process";
import {readFile} from "node:fs/promises";
import {fileURLToPath} from "node:url";
const root=fileURLToPath(new URL("../../../",import.meta.url));
const out=root+".workspace-build/cpu-bundle";
await build({configFile:root+"packages/workspace-ui/vite.config.ts",root:root+"packages/workspace-ui",logLevel:"error",build:{outDir:out,emptyOutDir:true,rollupOptions:{input:root+"packages/workspace-ui/test/performance.html"}}});
const server=createServer(async(req,res)=>{
 try {
  const path=new URL(req.url,"http://localhost").pathname;
  if(path.includes("..")) throw Error("Invalid path");
  const body=await readFile(out+(path==="/"?"/test/performance.html":path));
  res.setHeader("Content-Type",path.endsWith(".js")?"text/javascript":path.endsWith(".css")?"text/css":"text/html");res.end(body);
 } catch {res.writeHead(404);res.end();}
});
try {
 await new Promise(resolve=>server.listen(0,"127.0.0.1",resolve));
 const child=spawn("/usr/bin/python3",["-B",root+"packages/workspace-ui/test/webkit-cpu.py","http://127.0.0.1:"+server.address().port+"/"],{stdio:"inherit"});
 process.exitCode=await new Promise((resolve,reject)=>{child.on("error",reject);child.on("exit",code=>resolve(code??1));});
} finally {await new Promise(resolve=>server.close(resolve));}
