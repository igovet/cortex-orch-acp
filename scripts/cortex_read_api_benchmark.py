#!/usr/bin/env python3
"""Deterministic JSON byte/round-trip examples, not a live token/cost benchmark."""
from pathlib import Path
import json
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from cortex_runtime.channeled_tools import channeled_base_tools
from cortex_runtime.delegation.mcp_tools import tool_definitions
from cortex_runtime.engine.mcp_bridge import always_load
from cortex_runtime.tool_read_api import encoded


def measurements():
    rows = []
    for kind, role, stage in [
        ("coordinator", None, None),
        ("worker", "worker-backend-dev", "execution"),
        ("worker", "worker-architect", "execution"),
        ("worker", "worker-code-reviewer", "review"),
        ("worker", "worker-senior-consultant", "execution"),
    ]:
        tools = sorted([always_load(tool) for tool in list(channeled_base_tools()) +
                        tool_definitions(kind, role_id=role, stage=stage)], key=lambda tool: tool["name"])
        report = next(tool for tool in tools if tool["name"] == "read_report")
        rows.append(dict(profile=role or kind, offered_tools=len(tools),
                         catalogue_bytes=len(encoded(tools)),
                         named_read_report_bytes=len(encoded(dict(catalog_revision="0"*64, tool=report)))))
    # Literal fixture references, not retained publications or execution evidence.
    calls = [dict(id=str(index), tool="read_report",
                  arguments=dict(report_id=f"r_{index:012x}", limit=1000)) for index in range(1, 4)]
    return dict(
        basis="deterministic compact UTF-8 JSON fixtures; no live engine or tokens measured",
        catalogues=rows,
        three_reads=dict(individual_argument_bytes=sum(len(encoded(call["arguments"])) for call in calls),
                         batch_argument_bytes=len(encoded(dict(calls=calls))),
                         serial_client_round_trips=3, batch_round_trips=1,
                         discover_then_batch_round_trips=2, native_exec_round_trips=1),
    )


if __name__ == "__main__":
    print(json.dumps(measurements(), ensure_ascii=False, indent=2, sort_keys=True))
