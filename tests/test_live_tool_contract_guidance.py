"""Regression coverage for arguments confused during live coordinator work."""

import pytest

from cortex_runtime.contracts import BY_NAME, StoreError, validate as validate_report_tool
from cortex_runtime.delegation.contracts import validate as validate_delegation_tool
from cortex_runtime.delegation.mcp_tools import tool_definitions


def test_read_report_rejects_write_delivery_key_with_schema_correction():
    with pytest.raises(StoreError) as caught:
        validate_report_tool("read_report", {"request_key": "read-1"})
    error = caught.value
    assert error.field == "request_key"
    assert error.received == "unadvertised field(s)"
    assert "tools/list" in error.correction


def test_list_events_rejects_oversized_page_with_advertised_bound():
    with pytest.raises(StoreError) as caught:
        validate_delegation_tool(
            "coordinator", "list_events", {"limit": 300}
        )
    error = caught.value
    assert error.field == "limit"
    assert error.expected == "1 through 200"
    assert "advertised bound" in error.correction


def test_list_events_read_schema_accepts_the_live_empty_input():
    schema = next(tool["inputSchema"] for tool in tool_definitions("coordinator")
                  if tool["name"] == "list_events")
    assert schema["required"] == []
    assert "request_key" not in schema["properties"]
    assert validate_delegation_tool("coordinator", "list_events", {}) == {}


def test_publish_result_schema_maps_write_report_receipt_name_to_input_name():
    published = next(tool for tool in tool_definitions("worker")
                     if tool["name"] == "publish_result")
    report_ref = published["inputSchema"]["properties"]["report_ref"]["description"]

    assert "write_report" in report_ref
    assert "report_id" in report_ref
    assert "report_ref" in report_ref
    assert "copy that receipt's report_id unchanged into this field" in report_ref
    assert "report_id" in published["description"]
    assert "report_ref" in published["description"]
    assert "Include the `checks` array" in published["description"]
    checks = published["inputSchema"]["properties"]["checks"]["description"]
    assert "one item for every acceptance check" in checks
    assert "empty array only when the assignment has no evidence-checkable" in checks


def test_read_report_schema_maps_result_report_ref_to_read_report_id():
    read = BY_NAME["read_report"]
    description = read["description"]
    report_id = read["inputSchema"]["properties"]["report_id"]["description"]

    assert "worker_result or publish_result" in description
    assert "copy that exact value into this tool's report_id argument" in description
    assert "report_ref is not an accepted read_report argument" in description
    assert "worker_result or publish_result" in report_id
    assert "report_ref value" in report_id
    assert "report_ref is not an accepted argument name" in report_id


def test_read_report_rejects_report_ref_with_exact_field_correction():
    with pytest.raises(StoreError) as caught:
        validate_report_tool("read_report", {"report_ref": "r_0123456789ab"})
    error = caught.value
    assert error.field == "report_ref"
    assert error.expected == "report_id (optional)"
    assert "worker_result.report_ref" in error.correction
    assert "read_report.report_id" in error.correction


def test_write_report_schema_names_every_draft_placeholder_for_removal():
    description = BY_NAME["write_report"]["description"]
    assert "replace every exact line in replaceable_markers" in description
    assert "including guidance comments" in description
    assert "required_replacement_count" in description
    assert "native file read/search" in description
    assert "server will reject the publication" in description


def test_create_draft_contract_requires_zero_marker_preflight_before_publish():
    description = BY_NAME["create_draft"]["description"]
    assert "final native-file exact search" in description
    assert "every returned replaceable_markers entry" in description
    assert "a write_report call with even one exact marker still present is guaranteed to fail" in description
