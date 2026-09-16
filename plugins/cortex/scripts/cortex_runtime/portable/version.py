"""Portable contract version and compatibility policy.

``CONTRACT_VERSION`` is an opaque integer that a consumer compares for exact
equality against the version it was built for; it is not a semantic-version
string and carries no ordering guarantee beyond "same or different".

Bump ``CONTRACT_VERSION`` whenever a change could break an existing consumer
built against the prior version, including:
  - Removing a public name (module, class, function, dataclass field, or
    Enum member) from this package.
  - Changing an existing function or method signature: a parameter removed,
    reordered, renamed, or its accepted type narrowed.
  - Changing an existing field's type or semantics, or an Enum member's
    string value.
  - Changing validation to reject input that was previously accepted.

Do NOT bump ``CONTRACT_VERSION`` for additive, backward-compatible changes:
  - Adding a new class, function, dataclass field with a safe default, or
    Enum member.
  - Adding a new optional keyword-only parameter with a default value.
  - Widening accepted input (for example, accepting a new valid value that
    was previously rejected).

A consumer should treat an unexpected ``CONTRACT_VERSION`` -- lower or
higher than the version it was built against -- as incompatible until that
mismatch is explicitly reviewed; it must never silently proceed.
"""
from __future__ import annotations

CONTRACT_VERSION = 1
