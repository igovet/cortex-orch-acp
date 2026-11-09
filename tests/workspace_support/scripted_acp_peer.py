"""Test-only newline-delimited scripted ACP peer; no engines or host integrations.

The first stdin line is a JSON array of steps. A step can expect an exact
incoming frame, send JSON frames, inject a raw malformed line, or stop replying
until stdin closes. Mismatches exit nonzero without echoing input bodies.
"""
import json
import sys


def run(source, destination):
    steps = json.loads(source.readline())
    for step in steps:
        if "expect" in step:
            incoming = source.readline()
            if not incoming or json.loads(incoming) != step["expect"]:
                raise ValueError("script_mismatch")
        for frame in step.get("send", []):
            destination.write(json.dumps(frame, ensure_ascii=False) + "\n")
        if "raw" in step:
            destination.write(step["raw"] + "\n")
        destination.flush()
        if step.get("stopResponding"):
            for _ in source:
                pass
            return


if __name__ == "__main__":
    try:
        run(sys.stdin, sys.stdout)
    except (ValueError, KeyError, TypeError):
        print("scripted_peer_error", file=sys.stderr)
        sys.exit(2)
