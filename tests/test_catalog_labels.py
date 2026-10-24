"""Safe route label construction and disambiguation (T007)."""
from __future__ import annotations

import pytest

from cortex_runtime.catalog.labels import (
    ENGINE_SHORT_NAMES, build_connection_label, build_label, build_provider_label,
    disambiguate_labels, engine_short_name,
)
from cortex_runtime.portable.errors import PortableError


def test_label_format_is_engine_connection_model():
    assert build_label("codex-acp", "Work", "model-a") == "codex-acp · Work · model-a"


def test_label_strips_control_and_bidi_characters():
    poisoned = "Work‮Evil"
    label = build_label("codex-acp", poisoned, "model-a")
    assert "‮" not in label and "" not in label
    assert "WorkEvil" in label


def test_label_collapses_whitespace():
    label = build_label("codex-acp", "My   Team   Account", "model-a")
    assert "My Team Account" in label


def test_label_component_is_truncated_with_ellipsis():
    long_name = "x" * 200
    label = build_label("codex-acp", long_name, "model-a")
    component = label.split(" · ")[1]
    assert len(component) == 64
    assert component.endswith("…")


def test_label_rejects_url_shaped_connection_name():
    with pytest.raises(PortableError) as excinfo:
        build_label("codex-acp", "https://internal.example.com/token=abc", "model-a")
    assert excinfo.value.code == "unsafe_connection_label"


def test_label_rejects_hostname_shaped_connection_name():
    with pytest.raises(PortableError) as excinfo:
        build_label("codex-acp", "gateway.internal.example.com", "model-a")
    assert excinfo.value.code == "unsafe_connection_label"


def test_label_rejects_ipv4_shaped_connection_name():
    with pytest.raises(PortableError) as excinfo:
        build_label("codex-acp", "10.0.0.5", "model-a")
    assert excinfo.value.code == "unsafe_connection_label"


def test_label_rejects_credential_shaped_connection_name():
    with pytest.raises(PortableError) as excinfo:
        build_label("codex-acp", "user@internal-host", "model-a")
    assert excinfo.value.code == "unsafe_connection_label"


def test_label_allows_plain_model_id_with_a_dot():
    # A model id like "gpt-4.1" must not be rejected as hostname-shaped.
    assert "gpt-4.1" in build_label("codex-acp", "Work", "gpt-4.1")


def test_label_rejects_bare_ipv6_connection_name():
    with pytest.raises(PortableError) as excinfo:
        build_label("codex-acp", "2001:db8::1", "model-a")
    assert excinfo.value.code == "unsafe_connection_label"


def test_label_rejects_bracketed_ipv6_connection_name():
    with pytest.raises(PortableError) as excinfo:
        build_label("codex-acp", "[2001:db8::1]:443", "model-a")
    assert excinfo.value.code == "unsafe_connection_label"


def test_label_rejects_loopback_ipv6_connection_name():
    with pytest.raises(PortableError) as excinfo:
        build_label("codex-acp", "::1", "model-a")
    assert excinfo.value.code == "unsafe_connection_label"


def test_label_rejects_scheme_less_host_port_connection_name():
    with pytest.raises(PortableError) as excinfo:
        build_label("codex-acp", "internal-gateway:8080", "model-a")
    assert excinfo.value.code == "unsafe_connection_label"


def test_label_rejects_scheme_less_host_port_with_path_connection_name():
    with pytest.raises(PortableError) as excinfo:
        build_label("codex-acp", "internal.example.com:8443/v1/api", "model-a")
    assert excinfo.value.code == "unsafe_connection_label"


def test_label_rejects_ipv4_port_connection_name():
    with pytest.raises(PortableError) as excinfo:
        build_label("codex-acp", "10.0.0.5:9090", "model-a")
    assert excinfo.value.code == "unsafe_connection_label"


def test_label_rejects_punycode_connection_name():
    with pytest.raises(PortableError) as excinfo:
        build_label("codex-acp", "xn--80ak6aa92e.com", "model-a")
    assert excinfo.value.code == "unsafe_connection_label"


def test_label_rejects_digit_tld_hostname_connection_name():
    with pytest.raises(PortableError) as excinfo:
        build_label("codex-acp", "internal.c0m", "model-a")
    assert excinfo.value.code == "unsafe_connection_label"


def test_label_rejects_userinfo_credential_form_connection_name():
    with pytest.raises(PortableError) as excinfo:
        build_label("codex-acp", "svc-account:secrettoken@proxy.internal", "model-a")
    assert excinfo.value.code == "unsafe_connection_label"


def test_label_strips_zero_width_and_invisible_format_characters():
    poisoned = "Wo​rk⁠Account﻿"
    label = build_label("codex-acp", poisoned, "model-a")
    for invisible in ("​", "⁠", "﻿"):
        assert invisible not in label
    assert "WorkAccount" in label


def test_label_applies_nfkc_normalization_before_checks():
    # Fullwidth colon (U+FF1A) NFKC-normalizes to ASCII ':', so a fullwidth
    # lookalike of a host:port pair must be caught, not treated as safe text.
    poisoned = "internal-gateway：8080"
    with pytest.raises(PortableError) as excinfo:
        build_label("codex-acp", poisoned, "model-a")
    assert excinfo.value.code == "unsafe_connection_label"


def test_label_rejects_ipv6_zone_id_bypass():
    with pytest.raises(PortableError) as excinfo:
        build_label("codex-acp", "fe80::1%eth0", "model-a")
    assert excinfo.value.code == "unsafe_connection_label"


def test_label_rejects_ipv4_mapped_ipv6_bypass():
    with pytest.raises(PortableError) as excinfo:
        build_label("codex-acp", "::ffff:192.168.1.1", "model-a")
    assert excinfo.value.code == "unsafe_connection_label"


def test_label_rejects_spaced_colon_host_port_bypass():
    with pytest.raises(PortableError) as excinfo:
        build_label("codex-acp", "example.com : 8080", "model-a")
    assert excinfo.value.code == "unsafe_connection_label"


def test_label_rejects_hidden_combining_mark_bypass():
    # U+034F COMBINING GRAPHEME JOINER is category Mn, not Cf, so it survives
    # invisible-format stripping; stacked directly on an ASCII base ('a') it
    # must still be rejected by the combining-mark-after-ASCII rule.
    poisoned = "exa͏mple.com"
    with pytest.raises(PortableError) as excinfo:
        build_label("codex-acp", poisoned, "model-a")
    assert excinfo.value.code == "unsafe_connection_label"


def test_label_rejects_combining_mark_at_start():
    with pytest.raises(PortableError) as excinfo:
        build_label("codex-acp", "́Work", "model-a")
    assert excinfo.value.code == "unsafe_connection_label"


@pytest.mark.parametrize("name", [
    "Work account 2",
    "Team: research",
    "Personal (Claude)",
    "Проект Альфа",
    "हिन्दी टीम",
])
def test_label_allows_lead_specified_connection_names(name):
    label = build_label("codex-acp", name, "model-a")
    component = label.split(" · ")[1]
    assert component == name


@pytest.mark.parametrize("name", [
    "example.com",
    "192.168.1.1",
    "fe80::1%eth0",
    "::ffff:192.168.1.1",
    "example.com : 8080",
    "exa͏mple.com",
    "user@host",
    "https://example.com",
    "host/path",
    "host\\path",
    "50%",
    "a[b]",
    "a=b",
    "a?b",
    "a*b",
])
def test_label_rejects_disallowed_characters_and_shapes(name):
    with pytest.raises(PortableError) as excinfo:
        build_label("codex-acp", name, "model-a")
    assert excinfo.value.code == "unsafe_connection_label"


def test_label_model_component_keeps_dots_and_url_like_text_but_strips_invisibles():
    # engine/model are sanitized (never rejected): a model id keeps its dots,
    # and any URL/IP-like text is passed through sanitized rather than
    # blocked, per the lead's conservative decision for non-connection-name
    # components.
    label = build_label("codex-acp", "Work", "model-1.5")
    assert "model-1.5" in label
    poisoned_model = "http://example.com/model​"
    label = build_label("codex-acp", "Work", poisoned_model)
    assert "​" not in label
    assert "http://example.com/model" in label


def test_label_model_component_strips_combining_mark_after_ascii_base():
    label = build_label("codex-acp", "Work", "módel-a")
    assert "́" not in label


def test_engine_short_name_maps_known_adapter_ids_to_their_short_name():
    assert engine_short_name("claude-agent-acp") == "claude"
    assert engine_short_name("codex-acp") == "codex"
    # An unmapped engine id falls back to itself, never a fabricated guess.
    assert engine_short_name("future-engine-acp") == "future-engine-acp"
    # Every engine in connections.model.SUPPORTED_ENGINES has exactly one
    # entry here -- a new engine only needs that (T035 review requirement).
    from cortex_runtime.connections.model import SUPPORTED_ENGINES

    assert set(ENGINE_SHORT_NAMES) == set(SUPPORTED_ENGINES) == {"claude-agent-acp", "codex-acp", "opencode"}
    assert engine_short_name("opencode") == "opencode"


def test_build_connection_label_uses_the_short_engine_name_not_the_adapter_package_id():
    assert build_connection_label("claude-agent-acp", "claude-sonnet-live") == "claude · claude-sonnet-live"
    assert build_connection_label("codex-acp", "codex-luna") == "codex · codex-luna"


# --- build_provider_label: the first selector shows the real account, never
# the operator-chosen nickname (T035 account-label follow-up) -----------

def test_build_provider_label_shows_the_resolved_account():
    assert build_provider_label("codex-acp", "user+2@example.com") == "codex · user+2@example.com"
    assert build_provider_label("claude-agent-acp", "user@example.com") == "claude · user@example.com"


def test_build_provider_label_falls_back_to_engine_alone_without_an_account():
    assert build_provider_label("codex-acp", None) == "codex"
    assert build_provider_label("claude-agent-acp", "") == "claude"


def test_build_provider_label_never_shows_the_raw_adapter_package_id():
    label = build_provider_label("codex-acp", "user@example.com")
    assert "codex-acp" not in label


def test_build_provider_label_sanitizes_but_never_rejects_the_account():
    poisoned = "user‮evil@example.com"
    label = build_provider_label("codex-acp", poisoned)
    assert "‮" not in label
    assert "userevil@example.com" in label


def test_build_provider_label_two_connections_same_engine_different_accounts_distinguishable():
    first = build_provider_label("codex-acp", "user@example.com")
    second = build_provider_label("codex-acp", "user+1@example.com")
    assert first != second


def test_disambiguate_labels_appends_route_suffix_only_on_collision():
    entries = [("xr_" + "a" * 24, "codex-acp · Work · model-a"),
               ("xr_" + "b" * 24, "codex-acp · Work · model-a"),
               ("xr_" + "c" * 24, "codex-acp · Work · model-b")]
    result = disambiguate_labels(entries)
    assert result["xr_" + "a" * 24] != result["xr_" + "b" * 24]
    assert result["xr_" + "a" * 24].startswith("codex-acp · Work · model-a (#")
    assert result["xr_" + "c" * 24] == "codex-acp · Work · model-b"
