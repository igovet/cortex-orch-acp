"""A store refusal the user can act on must say what to fix.

Found live: a `.cortex` directory that was not owner-only made every new
Cortex thread in that project fail with "Storage failed while handling this
request." -- a one-second fix presented as an opaque internal error, with
the actual reason (``unsafe_storage``, mode 0700 expected) already sitting
in the exception and thrown away on its way to the client.
"""
from __future__ import annotations

import os

import pytest

from cortex_runtime.acp.agent import _store_error_message
from cortex_runtime.contracts import StoreError
from cortex_runtime.store import private_directory


def test_the_message_names_the_field_the_value_expected_mode_and_fix_command():
    message = _store_error_message(StoreError(
        "unsafe_storage", "storage_path", "/p/.cortex", "owner-only directory mode 0700",
        "run chmod 700 /p/.cortex",
    ))
    assert "unsafe_storage" in message
    assert "storage_path" in message
    assert "/p/.cortex" in message
    assert "owner-only directory mode 0700" in message
    assert "run chmod 700 /p/.cortex" in message


def test_a_refusal_carrying_nothing_but_a_code_still_reads_as_a_sentence():
    assert _store_error_message(StoreError("busy")) == (
        "Storage failed while handling this request: busy."
    )


def test_existing_group_readable_store_directory_has_an_actionable_fix(tmp_path):
    directory = tmp_path / ".cortex"
    directory.mkdir(mode=0o700)
    os.chmod(directory, 0o755)
    with pytest.raises(StoreError) as error:
        private_directory(directory)
    assert error.value.field == "storage_path"
    assert error.value.received == str(directory)
    assert error.value.expected == "owner-only directory mode 0700"
    assert error.value.correction == f"run chmod 700 {directory}"
