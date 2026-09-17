"""ResourcePolicy: opt-in host-capability advertisement for T014's bridges.

Default policy advertises nothing (``fs_local=False``, ``terminal=False``):
:meth:`ResourcePolicy.client_capabilities` returns an empty ``{}`` object in
that case, exactly matching T012's pre-T014 ``initialize`` behavior (see
``session_manager.EngineSessionManager.negotiate``) -- existing tests and
callers that never opt in are unaffected.
"""
from __future__ import annotations

from dataclasses import dataclass

DEFAULT_OUTPUT_BYTE_LIMIT_MAX = 1024 * 1024  # 1 MiB


@dataclass(frozen=True)
class ResourcePolicy:
    fs_local: bool = False
    terminal: bool = False
    output_byte_limit_max: int = DEFAULT_OUTPUT_BYTE_LIMIT_MAX

    def __post_init__(self) -> None:
        if not isinstance(self.output_byte_limit_max, int) or self.output_byte_limit_max <= 0:
            raise ValueError("output_byte_limit_max must be a positive int.")

    def client_capabilities(self) -> dict:
        """The ACP ``clientCapabilities`` object this policy advertises on
        ``initialize``. Permission requests are always implicitly supported
        by an ACP client (there is no separate capability flag for them);
        this only ever gates ``fs``/``terminal``."""
        caps: dict = {}
        if self.fs_local:
            caps["fs"] = {"readTextFile": True, "writeTextFile": True}
        if self.terminal:
            caps["terminal"] = True
        return caps
