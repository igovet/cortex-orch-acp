"""Narrow internal delegation operations and their atomic provisioning.

These are Cortex-internal service operations, not new standard ACP methods
(see ``contracts.py`` for the actual advertised MCP tool set and
``acp/protocol.py`` for the ACP methods this process handles/sends). Nothing
here is exposed on the wire under a "session/..." or other ACP-reserved
method name.
"""
