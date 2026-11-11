// The sprint Graph view in WebKitGTK: captures at four window sizes, a region-overlap check and frame times on 200 tasks.
// Same static bundle, fake data and container as layout-fit.mjs. Evidence goes to the git-ignored .design-evidence/sprint-graph-product.
import {build} from "vite";
import {createServer} from "node:http";
import {spawn} from "node:child_process";
import {readFile, mkdir} from "node:fs/promises";
import {fileURLToPath} from "node:url";
const root=fileURLToPath(new URL("../../../",import.meta.url));
const out=root+".workspace-build/sprint-graph-render";
const shots=root+".design-evidence/sprint-graph-product";
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
 const child=spawn("timeout",["--kill-after=5","1200","docker","run","--rm","-i","--network","host","--user",process.getuid()+":"+process.getgid(),"-e","HOME=/tmp","-v",shots+":/evidence","cortex-workspace-rendered:ubuntu26.04","python3","-B","-",url,...process.argv.slice(2)],{stdio:["pipe","inherit","inherit"]});
 child.stdin.end(await readFile(new URL("./sprint-graph-render.py",import.meta.url)));
 process.exitCode=await new Promise((resolve,reject)=>{child.on("error",reject);child.on("exit",code=>resolve(code??1));});
}finally{await new Promise(resolve=>server.close(resolve));}
