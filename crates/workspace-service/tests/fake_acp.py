"""Disposable ACP peer: never imports an engine or reads user configuration."""
import json
import os
import sys
import time

mode = sys.argv[1]
held = []
journal_sent = False
for line in sys.stdin:
    request = json.loads(line)
    method, params = request["method"], request.get("params", {})
    with open("fake.trace", "a") as trace:
        trace.write(params.get("operation", method) + "\n")
    if method == "session/cancel":
        continue
    if method == "initialize":
        if mode == "slow_init":
            time.sleep(1.2)
        if mode == "once" and os.path.exists("started-once"):
            os._exit(3)
        if mode == "once":
            open("started-once", "w").close()
        result = {"protocolVersion": 1}
    elif method == "session/new" and mode == "refuse_session":
        print(json.dumps(dict(jsonrpc="2.0", id=request["id"], error={"code": -32000, "message": "private diagnostic"})), flush=True)
        continue
    elif method == "session/new":
        result = {"sessionId": "template", "workspaceActivation": {"outcome": "recorded"}}
    elif method == "_cortex/workspace":
        operation = params["operation"]
        if operation == "capabilities.get":
            # The service validates and negotiates these against its own schema.
            from pathlib import Path
            schema = json.loads(Path(sys.argv[2]).read_text())
            import hashlib
            fingerprint = hashlib.sha256(json.dumps(schema, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()
            result = dict(protocol={"major": 1, "minor": 0}, buildIdentity="fake",
                          generation="1", schemaFingerprint=fingerprint, mappingVersion="1",
                          operations=schema["x-operations"], limits=schema["x-limits"], acpCapabilities={})
        elif operation == "project.snapshot":
            result = {"projectId": params["projectId"], "version": "1", "tasks": {"items": [], "nextCursor": None, "highWatermark": "0"}, "highWatermark": "0", "eventCursor": "fake0"}
        elif operation == "events.page":
            items = []
            if mode == "journal" and not journal_sent:
                items = [dict(eventId="event-1", sequence="1", entityId="task-1", entityVersion="1", kind="task", recordedAt="now")]
                journal_sent = True
            result = {"items": items, "nextCursor": None, "highWatermark": "1" if mode == "journal" else "0",
                      "eventCursor": "fake1" if mode == "journal" else "fake0"}
        elif operation == "tasks.get" and mode == "reverse":
            held.append(request)
            if len(held) < 2:
                continue
            for item in reversed(held):
                print(json.dumps(dict(jsonrpc="2.0", id=item["id"], result={"target": item["params"]["target"]})), flush=True)
            held = []
            continue
        elif operation == "tasks.get" and mode == "host":
            print(json.dumps(dict(jsonrpc="2.0", id="permission", method="session/request_permission", params={})), flush=True)
            response = json.loads(sys.stdin.readline())
            result = {"refused": response["id"] == "permission" and response.get("error", {}).get("code") == -32601}
        elif operation == "tasks.get" and mode == "oversized":
            sys.stdout.write("x" * 2100000 + "\n")
            sys.stdout.flush()
            from pathlib import Path
            Path("oversize-drained").write_text("drained")
            result = {}
        elif operation == "tasks.get" and mode in ("exit", "once"):
            os._exit(7)
        elif operation == "tasks.get" and mode == "stall_once" and not os.path.exists("stalled"):
            open("stalled", "w").close()
            time.sleep(3)
            result = {"late": True}
        elif operation == "tasks.get" and mode == "bad_id":
            print(json.dumps(dict(jsonrpc="2.0", id="wrong-type", result={})), flush=True)
            continue
        elif operation == "tasks.get" and mode in ("flood", "single"):
            for n in range(200 if mode == "flood" else 1):
                print(json.dumps(dict(jsonrpc="2.0", method="session/update", params={"n": n})), flush=True)
            result = {}
        elif operation == "tasks.get" and mode == "stderr":
            sys.stderr.write("x" * 200000)
            sys.stderr.flush()
            result = {"drained": True}
        elif operation == "tasks.get" and mode == "hang":
            time.sleep(60)
        else:
            allowed = {"HOME", "PATH", "LANG", "LC_ALL", "LC_CTYPE", "XDG_CONFIG_HOME", "XDG_STATE_HOME",
                       "XDG_CACHE_HOME", "XDG_RUNTIME_DIR", "XDG_DATA_HOME", "PYTHONDONTWRITEBYTECODE", "CORTEX_WORKSPACE_STATE_DIR",
                       "CORTEX_WORKSPACE_CHANNEL"}
            result = {"actor": params.get("actorId"), "pid": os.getpid(), "unexpectedEnvironment": sorted(set(os.environ) - allowed),
                      "channel": os.environ.get("CORTEX_WORKSPACE_CHANNEL"),
                      "devEnvironment": all(os.environ.get(key, "").endswith(suffix) for key, suffix in
                                            (("XDG_CONFIG_HOME", "/state/config"), ("XDG_STATE_HOME", "/state/state"),
                                             ("XDG_CACHE_HOME", "/state/cache"), ("XDG_DATA_HOME", "/state/data")))}
    else:
        result = {}
    print(json.dumps(dict(jsonrpc="2.0", id=request["id"], result=result)), flush=True)
