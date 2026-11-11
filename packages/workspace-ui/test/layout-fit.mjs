// The rendered layout gate: static production bundle, fake data, no watchers or engines. Add a screen with one entry in layout-fit.py.
import {build} from "vite";
import {createServer} from "node:http";
import {spawn} from "node:child_process";
import {readFile, mkdir} from "node:fs/promises";
import {fileURLToPath} from "node:url";
const root=fileURLToPath(new URL("../../../",import.meta.url));
const label=process.argv[2]??"layout";
if(!/^[a-z0-9-]+$/.test(label))throw Error("Use a simple evidence label");
// One build per label, so two runs with different labels never serve each other's bundle.
const out=root+".workspace-build/layout-text/"+label;
// Captures go to the git-ignored evidence folder, one folder per label. A problem makes this exit nonzero.
const shots=root+".design-evidence/layout-rebuild/"+label;
await mkdir(shots,{recursive:true});
// The smallest window the desktop app allows, from its own source, so the gate measures what the owner can shrink it to.
const minimum=(await readFile(root+"apps/cortex-workspace-desktop/src-tauri/src/desktop.rs","utf8")).match(/\.min_inner_size\(\s*(\d+)(?:\.0)?\s*,\s*(\d+)(?:\.0)?\s*\)/);
if(!minimum)throw Error("The desktop app's minimum window size was not found in desktop.rs");
// The widest the owner can drag the sidebar, which leaves the title band its least room.
const sidebar=(await readFile(root+"packages/workspace-ui/src/prefs.ts","utf8")).match(/SIDEBAR_MAX = (\d+)/);
if(!sidebar)throw Error("SIDEBAR_MAX was not found in prefs.ts");
await build({configFile:root+"packages/workspace-ui/vite.config.ts",root:root+"packages/workspace-ui",logLevel:"error",build:{outDir:out+"/bundle",emptyOutDir:true,rollupOptions:{input:root+"packages/workspace-ui/test/layout-fit.html"}}});
const server=createServer(async(req,res)=>{
 try {
  const path=new URL(req.url,"http://localhost").pathname;
  if(path.includes(".."))throw Error("Invalid path");
  // The desktop page as the engine first paints it: its own markup and public files, with no module able to run.
  const desktop=root+"apps/cortex-workspace-desktop/";
  const body=path==="/boot.html"?(await readFile(desktop+"index.html","utf8")).replace(/<script type="module"[^>]*><\/script>/,""):path==="/cortex-256.png"||path==="/theme-boot.js"||path==="/boot.css"?await readFile(desktop+"public"+path):await readFile(out+"/bundle"+(path.startsWith("/assets/")?path:"/test/layout-fit.html"));
  res.setHeader("Content-Type",path.endsWith(".js")?"text/javascript":path.endsWith(".css")?"text/css":path.endsWith(".png")?"image/png":"text/html");
  res.end(body);
 } catch {res.writeHead(404);res.end();}
});
try {
 await new Promise(resolve=>server.listen(0,"127.0.0.1",resolve));
 const url="http://127.0.0.1:"+server.address().port+"/";
 // The rule for boxed labels (one line, capitals in the middle) is shared with the page captures.
 const labelFit=await readFile(new URL("./label-fit.js",import.meta.url),"utf8");
 const child=spawn("timeout",["--kill-after=5","3000","docker","run","--rm","-i","--network","host","--user",process.getuid()+":"+process.getgid(),"-e","HOME=/tmp","-e","CW_MIN_WINDOW="+minimum[1]+"x"+minimum[2],"-e","CW_SIDEBAR_MAX="+sidebar[1],"-e","CW_LABEL_FIT="+labelFit,"-v",shots+":/evidence","cortex-workspace-rendered:ubuntu26.04","python3","-B","-",url,label,...process.argv.slice(3)],{stdio:["pipe","inherit","inherit"]});
 child.stdin.end(await readFile(new URL("./layout-fit.py",import.meta.url)));
 process.exitCode=await new Promise((resolve,reject)=>{child.on("error",reject);child.on("exit",code=>resolve(code??1));});
}finally{await new Promise(resolve=>server.close(resolve));}
