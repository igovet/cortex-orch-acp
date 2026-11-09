"""Benign stdio MCP listing fixture. Any tools/call is an explicit failure."""
import json
import sys

for line in sys.stdin:
    message = json.loads(line)
    if "id" not in message:
        continue
    method = message.get("method")
    if method == "initialize":
        result = {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}},
                  "serverInfo": {"name": "audit-discovery", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": [{"name": "audit_read", "description": "Listing only.",
                             "inputSchema": {"type": "object", "properties": {}}}]}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": message["id"],
                          "error": {"code": -32601, "message": "No tool execution allowed"}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}), flush=True)
