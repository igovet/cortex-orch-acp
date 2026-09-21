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


def test_source_pages_reject_oversized_reads():
    with pytest.raises(StoreError): validate_delegation_tool('coordinator','list_sources',dict(limit=300))


def test_source_read_schema_accepts_empty_arguments():
    assert validate_delegation_tool('coordinator','list_sources',{})=={}


def test_result_requires_exact_immutable_report_and_checks():
    published=next(tool for tool in tool_definitions('worker') if tool['name']=='result_submit')
    properties=published['inputSchema']['properties']
    assert 'write_report' in properties['report_ref']['description']
    assert 'report_id' in properties['report_ref']['description']
    assert 'checks' in published['inputSchema']['required']


def test_read_report_schema_maps_result_report_ref_to_read_report_id():
    read = BY_NAME["read_report"]
    description = read["description"]
    report_id = read["inputSchema"]["properties"]["report_id"]["description"]

    assert "worker_result or result_submit" in description
    assert "copy that exact value into this tool's report_id argument" in description
    assert "report_ref is not an accepted read_report argument" in description
    assert "worker_result or result_submit" in report_id
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


def test_write_report_uses_gateway_and_rejects_placeholders():
    description=BY_NAME['write_report']['description']
    assert 'draft_write' in description and 'placeholder' in description


def test_create_draft_returns_complete_owned_content_for_gateway():
    description=BY_NAME['create_draft']['description']
    assert 'draft_write' in description and 'complete initial Markdown' in description
