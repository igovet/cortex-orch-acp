"""No-paid regression for the Paseo 0.8 session/load compatibility shim."""
from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SHIM = ROOT / "integrations" / "paseo" / "server" / "acp-load-shim.mjs"


def test_load_replay_is_emitted_after_response_without_rewriting_frames(tmp_path):
    agent = tmp_path / "agent.py"
    agent.write_text(
        """
import json, sys
for line in sys.stdin:
    message = json.loads(line)
    if message.get('method') == 'initialize':
        print(json.dumps({'jsonrpc':'2.0','id':message['id'],'result':{'agentCapabilities':{'loadSession':True}}}), flush=True)
    elif message.get('method') == 'session/load':
        print(json.dumps({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'native-1','update':'history'}}), flush=True)
        print(json.dumps({'jsonrpc':'2.0','id':message['id'],'result':{'modes':None,'configOptions':[]}}), flush=True)
""".strip()
        + "\n",
        encoding="utf-8",
    )
    env = dict(os.environ)
    env["CORTEX_ACP_TARGET"] = json.dumps({"argv": [sys.executable, str(agent)]})
    process = subprocess.Popen(
        ["node", str(SHIM)],
        cwd=ROOT,
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    assert process.stdin is not None
    assert process.stdout is not None
    requests = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize"},
        {"jsonrpc": "2.0", "id": 2, "method": "session/load", "params": {"sessionId": "native-1"}},
    ]
    process.stdin.write("".join(json.dumps(request) + "\n" for request in requests))
    process.stdin.close()
    output = [json.loads(line) for line in process.stdout if line.strip()]
    process.wait(timeout=5)
    assert process.returncode == 0
    assert [frame.get("id") for frame in output] == [1, 2, None]
    assert output[2]["method"] == "session/update"
    assert output[2]["params"]["update"] == "history"
