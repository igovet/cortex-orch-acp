"""A store refusal the user can act on must say what to fix.

Found live: a `.cortex` directory that was not owner-only made every new
Cortex thread in that project fail with "Storage failed while handling this
request." -- a one-second fix presented as an opaque internal error, with
the actual reason (``unsafe_storage``, mode 0700 expected) already sitting
in the exception and thrown away on its way to the client.
"""
from __future__ import annotations

from cortex_runtime.acp.agent import _store_error_message
from cortex_runtime.contracts import StoreError


def test_the_message_names_the_field_the_value_and_what_was_expected():
    message = _store_error_message(StoreError(
        "unsafe_storage", "storage_path", "/p/.cortex", "owner-only directory mode 0700",
    ))
    assert "unsafe_storage" in message
    assert "storage_path" in message
    assert "/p/.cortex" in message
    assert "owner-only directory mode 0700" in message


def test_a_refusal_carrying_nothing_but_a_code_still_reads_as_a_sentence():
    assert _store_error_message(StoreError("busy")) == (
        "Storage failed while handling this request: busy."
    )
