// Captures of every page besides the task conversation in WebKitGTK, the desktop app's engine: production bundle, fake data
// (world=pages), a private virtual screen. Writes PNGs and a measurement file to the git-ignored
// .design-evidence/conversation/pages folder. It exits nonzero when a page scrolls sideways, a row mixes control heights, the
// product's name is shown, or a label wraps or sits off the middle of its box (label-fit.js); layout-fit.mjs remains the
// layout gate. `--set polish` captures every page at five window sizes in both themes into .design-evidence/conversation/polish.
import {build} from "vite";
import {createServer} from "node:http";
import {spawn} from "node:child_process";
import {readFile, mkdir} from "node:fs/promises";
import {fileURLToPath} from "node:url";
const root=fileURLToPath(new URL("../../../",import.meta.url));
const args=process.argv.slice(2);
const set=args[0]==="--set"?args[1]:"pages";
if(!/^(pages|polish|ui-fixes|review-fixes)$/.test(set))throw Error("Use --set pages, --set polish, --set ui-fixes or --set review-fixes");
const pages=args[0]==="--set"?args.slice(2):args;
const out=root+".workspace-build/pages-shots-"+set;
const shots=root+".design-evidence/conversation/"+set;
await mkdir(shots,{recursive:true});
await build({configFile:root+"packages/workspace-ui/vite.config.ts",root:root+"packages/workspace-ui",logLevel:"error",build:{outDir:out+"/bundle",emptyOutDir:true,rollupOptions:{input:root+"packages/workspace-ui/test/layout-fit.html"}}});
const labelFit=await readFile(new URL("./label-fit.js",import.meta.url),"utf8");
const server=createServer(async(req,res)=>{
 try {
  const path=new URL(req.url,"http://localhost").pathname;
  if(path.includes(".."))throw Error("Invalid path");
  const body=await readFile(out+"/bundle"+(path.startsWith("/assets/")?path:"/test/layout-fit.html"));
  res.setHeader("Content-Type",path.endsWith(".js")?"text/javascript":path.endsWith(".css")?"text/css":path.endsWith(".png")?"image/png":"text/html");
  res.end(body);
 } catch {res.writeHead(404);res.end();}
});
try {
 await new Promise(resolve=>server.listen(0,"127.0.0.1",resolve));
 const url="http://127.0.0.1:"+server.address().port+"/";
 const child=spawn("timeout",["--kill-after=5",set!=="pages"?"3600":"1500","docker","run","--rm","-i","--network","host","--user",process.getuid()+":"+process.getgid(),"-e","HOME=/tmp","-e","CW_SET="+set,"-e","CW_LABEL_FIT="+labelFit,"-v",shots+":/evidence","cortex-workspace-rendered:ubuntu26.04","python3","-B","-",url,...pages],{stdio:["pipe","inherit","inherit"]});
 child.stdin.end(await readFile(new URL("./pages-shots.py",import.meta.url)));
 process.exitCode=await new Promise((resolve,reject)=>{child.on("error",reject);child.on("exit",code=>resolve(code??1));});
}finally{await new Promise(resolve=>server.close(resolve));}
