"""Disposable real service/project. No native engine can be constructed by this launcher."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

root = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(root / "tests"))
from test_workspace_service_integration import Client, service_binary
binary = service_binary()
with tempfile.TemporaryDirectory(prefix="cw-feed-startup-") as temp:
    base = Path(temp)
    project = base / "projects" / "fixture"
    project.mkdir(parents=True)
    env = {k: v for k, v in os.environ.items() if k in ("PATH", "LANG", "LC_ALL")}
    home = base / "home"
    home.mkdir()
    for name in ("config", "state", "cache", "runtime"):
        (home / name).mkdir(mode=0o700)
    env.update(HOME=str(home), XDG_CONFIG_HOME=str(home / "config"), XDG_STATE_HOME=str(home / "state"),
               XDG_CACHE_HOME=str(home / "cache"), XDG_RUNTIME_DIR=str(home / "runtime"), PYTHONDONTWRITEBYTECODE="1")
    process = subprocess.Popen([str(binary), str(base / "projects"), sys.executable, str(root),
        "--fixture-launcher", str(root / "tests/test_workspace_service_integration.py")],
        env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    client = None
    try:
        sock = home / "runtime/cortex-workspace-dev/service.sock"
        deadline = time.monotonic() + 15
        while not sock.exists() and time.monotonic() < deadline:
            assert process.poll() is None, "service exited"
            time.sleep(.02)
        client = Client(sock)
        pid = client.call("projects.register", {"root": str(project), "displayName": "Disposable sprint project"})["projectId"]
        client.call("projects.activate", {"projectId": pid})
        def machine(op, **params):
            response = client.call("machine.call", {"op": op, "projectId": pid, "params": params})
            assert response["ok"], response.get("code")
            return response["result"]
        sprint = machine("product.sprint", action="create", description="Disposable mixed-state sprint", runMode="manual")["sprintId"]
        ids = {}
        for index, label in enumerate(("Backlog fixture", "Done fixture", "Planning fixture", "Sprint fixture", "Archived fixture")):
            reply = client.call("workspace", {"protocol":{"major":1,"minor":0},"requestId":"seed-"+str(index),"projectId":pid,
                "actorId":"fixture","operation":"tasks.create","target":pid,"expectedVersion":"0","ownerEpoch":None,
                "payload":{"description":label,"checkout":{"kind":"main"}}})
            ids[label] = reply["target"]
        for label in ("Planning fixture", "Sprint fixture"):
            machine("product.task", action="sprint", taskId=ids[label], sprintId=sprint, planning=label.startswith("Planning"))
        for label in ("Done fixture", "Archived fixture"):
            machine("product.task", action="close", taskId=ids[label], choice="as_is", confirmed=True)
        machine("product.task", action="merged", taskId=ids["Archived fixture"])
        machine("product.task", action="archive", taskId=ids["Archived fixture"])
        machine("product.sprint", action="patch", sprintId=sprint, patch={"taskIds":[ids["Sprint fixture"]],"planningTaskId":ids["Planning fixture"]})
        # The actual startup projection and product read intentionally disagree until combined.
        product = machine("product.read")
        assert product["tasks"][ids["Archived fixture"]].get("archive"), "fixture archive was not recorded"
        print(json.dumps({"socket":str(sock),"projectId":pid,"tasks":ids,"sprintId":sprint,
            "overlays":{key:{"planning":value.get("planning"),"status":value.get("status"),"archive":bool(value.get("archive"))} for key,value in product["tasks"].items()}}), flush=True)
        sys.stdin.readline()
        client.call("service.stop", {})
        assert process.wait(timeout=15) == 0
    finally:
        if client:
            client.close()
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
