"""Run owned source-only artifacts without host settings, install, network or credentials."""
import os
from pathlib import Path
import subprocess
import sys

ROOT=Path(__file__).resolve().parents[2]
OUT=ROOT/".workspace-build/stop-reports"
run=OUT/("run-"+sys.argv[1])
run.mkdir(parents=True,exist_ok=False)
command=["docker","run","--rm","--network=none","--shm-size=512m","--user",f"{os.getuid()}:{os.getgid()}",
    "--cap-drop=ALL","--security-opt=seccomp=unconfined","--security-opt=apparmor=unconfined"]
for source,target,readonly in ((OUT/"work","/checkout",True),(OUT/"pkg/usr/bin/cortex-workspace-dev","/usr/bin/cortex-workspace-dev",True),
    (OUT/"pkg/usr/bin/cortex-workspace-service-dev","/usr/bin/cortex-workspace-service-dev",True),
    (ROOT/"tests","/checkout/tests",True),(run,"/evidence",False)):
    command+=["--mount",f"type=bind,source={source},target={target}"+(",readonly" if readonly else "")]
command+=["--env","HOME=/tmp","cortex-workspace-rendered:ubuntu26.04","python3","-B",
    "/checkout/tests/workspace_support/stop_reports_native.py","--evidence","/evidence/observations"]
raise SystemExit(subprocess.run(command).returncode)
