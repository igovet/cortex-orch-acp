"""Command identity, deadlines and receipt authority are advertised contracts."""
from cortex_runtime.orchestration.gateway import SCHEMAS


def test_command_schema_requires_identity_and_leaves_runtime_unbounded_by_default():
    schema=SCHEMAS['command_run']
    assert schema['required']==['argv','request_key']
    assert 'maximum' not in schema['properties']['timeout_seconds']
    assert 'ambiguous interrupted calls' in schema['properties']['request_key']['description']
    assert schema['additionalProperties'] is False


def test_result_checks_reference_exact_command_receipts():
    from cortex_runtime.orchestration.contracts import RESULT
    check=RESULT['properties']['checks']['items']
    assert 'retained complete command result' in check['properties']['tool_call_ref']['description']
    assert set(check['properties']['outcome']['enum'])=={'passed','failed','not_run','unverified'}
