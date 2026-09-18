"""Connection modeling, onboarding, environment isolation, and trust.

Connections are modeled separately from engines/providers/routes (see
``portable.identity.Connection``/``ExecutionRoute``).  This package never
stores or logs a secret value; it only ever holds an opaque
``credential_ref`` pointing at an engine-native directory, a Cortex-managed
secret file, or an OS keyring entry, and it constructs least-privilege,
per-connection process environments on demand.
"""
from __future__ import annotations
