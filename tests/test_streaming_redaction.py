"""No-paid deterministic coverage for chunk-boundary redaction."""

from hashlib import sha256

import pytest

from cortex_runtime.observability.redact import StreamingRedactor, scrub_display_text, scrub_text


def _stream(parts):
    redactor = StreamingRedactor(max_chars=4000)
    emitted = [redactor.push(part) for part in parts]
    emitted.append(redactor.flush())
    return emitted


def _stream_with_metrics(parts, *, preserve_labeled_digests=False):
    redactor = StreamingRedactor(max_chars=4000)
    emitted = []
    max_pending = 0
    for part in parts:
        emitted.append(redactor.push(part))
        max_pending = max(max_pending, len(redactor._pending))
    emitted.append(redactor.flush(preserve_labeled_digests=preserve_labeled_digests))
    max_pending = max(max_pending, len(redactor._pending))
    return emitted, max_pending


def test_split_opaque_token_is_never_emitted_before_completion():
    secret = "abcde12345fghij67890klmno"
    emitted = _stream(["ordinary ", secret[:15], secret[15:], " tail"])
    assert emitted[1] == ""
    joined = "".join(emitted)
    assert secret not in joined
    assert joined == scrub_text("ordinary " + secret + " tail", max_chars=4000)


def test_hyphenated_script_name_survives_character_by_character_streaming():
    source = "includes/acf-blocks-v2-iframe-compat.php"
    redactor = StreamingRedactor(max_chars=4000)
    output = "".join(redactor.push(character) for character in source)
    output += redactor.flush()
    assert output == source


def test_coordinator_stream_preserves_path_embedded_script_names_character_by_character():
    source = "includes/a1b2c3d4e5f6g7h8i9j0k1l2.php"
    redactor = StreamingRedactor(max_chars=4000, preserve_paths=True)
    output = "".join(redactor.push(character) for character in source)
    output += redactor.flush()
    assert output == source


def test_coordinator_stream_preserves_hyphenated_underscored_and_digit_only_paths():
    paths = (
        "includes/acf-blocks-v2.php",
        "includes/acf_blocks_v2.php",
        "includes/123456789012345678901234567890.php",
    )
    for source in paths:
        redactor = StreamingRedactor(max_chars=4000, preserve_paths=True)
        output = "".join(redactor.push(character) for character in source)
        output += redactor.flush()
        assert output == source


def test_coordinator_stream_still_redacts_credentials_across_chunk_boundaries():
    secret = "abcde12345fghij67890klmno"
    source = f"includes/a1b2c3d4e5f6g7h8i9j0k1l2.php api_key={secret}"
    redactor = StreamingRedactor(max_chars=4000, preserve_paths=True)
    output = "".join(redactor.push(character) for character in source)
    output += redactor.flush()
    assert "includes/a1b2c3d4e5f6g7h8i9j0k1l2.php" in output
    assert secret not in output
    assert "api_key=[REDACTED]" in output


def test_split_key_value_and_private_path_preserve_exact_scrubbed_text():
    value = "api_key=123456789012345678901234567890 /private/12345678901234567890"
    emitted = _stream(["prefix ", value[:19], value[19:], " tail"])
    joined = "".join(emitted)
    assert "123456789012345678901234567890" not in joined
    assert joined == scrub_text("prefix " + value + " tail", max_chars=4000)


def test_sensitive_prefix_is_held_across_its_delimiter():
    for prefix in ("Bearer", "secret:"):
        secret = "abcdefghijklmno12345"
        emitted = _stream(["prefix " + prefix + " ", secret + " tail"])
        joined = "".join(emitted)
        assert secret not in joined


def test_labelled_numeric_digest_survives_stream_scrubbing_for_acceptance():
    digest = "1" * 64
    redactor = StreamingRedactor(max_chars=4000)
    emitted = [redactor.push(part) for part in ("Hash: ", digest, " tail")]
    assert digest not in "".join(emitted)
    assert digest in redactor.flush(preserve_labeled_digests=True)


def test_verified_digest_with_punctuation_is_preserved_only_when_accepted():
    digest = "1" * 64
    for suffix in (".", ",", ")", "]", ":"):
        redactor = StreamingRedactor(max_chars=4000)
        for part in ("Hash: ", digest + suffix):
            redactor.push(part)
        assert digest not in redactor.flush()
        redactor = StreamingRedactor(max_chars=4000)
        for part in ("Hash: ", digest + suffix):
            redactor.push(part)
        assert redactor.flush(preserve_labeled_digests=True) == "Hash: " + digest + suffix


def test_ordinary_chunks_stream_and_flush_is_not_duplicate():
    emitted = _stream(["Checking ", "in now. "])
    assert emitted[:2] == ["Checking ", "in now. "]
    assert emitted[-1] == ""
    assert "".join(emitted) == "Checking in now. "


def test_streamed_digest_is_over_exact_replayed_text():
    source = "prefix api_key=123456789012345678901234567890 tail"
    emitted = _stream([source[:12], source[12:28], source[28:]])
    replay = "".join(emitted)
    assert sha256(replay.encode()).hexdigest() == sha256(scrub_text(source, max_chars=4000).encode()).hexdigest()


def test_large_transport_chunk_matches_small_chunk_partition():
    source = "ordinary words " * 390
    assert len(source) == 5850
    one_chunk = _stream([source])
    hundred_chars = _stream([source[i:i + 100] for i in range(0, len(source), 100)])
    assert "[REDACTED_STREAM]" not in "".join(one_chunk)
    assert "".join(one_chunk) == "".join(hundred_chars)


def test_split_secret_remains_redacted_across_large_and_small_partitions():
    secret = "abcde12345fghij67890klmno"
    source = "prefix " + secret + " suffix"
    for parts in (
        [source],
        [source[:len(source) // 2], source[len(source) // 2:]],
        [source[i:i + 100] for i in range(0, len(source), 100)],
    ):
        output = "".join(_stream(parts))
        assert secret not in output
        assert output == scrub_text(source, max_chars=4000)


def test_labelled_digest_preservation_is_partition_invariant():
    digest = "1" * 64
    source = "Hash: " + digest + "."
    for parts in (
        [source],
        ["Hash: ", digest, "."],
        [source[i:i + 100] for i in range(0, len(source), 100)],
    ):
        redactor = StreamingRedactor(max_chars=4000)
        output = "".join(redactor.push(part) for part in parts)
        output += redactor.flush(preserve_labeled_digests=True)
        assert output == source


def test_split_sha256_label_never_releases_digest_before_acceptance():
    digest = "1" * 64
    source = "sha 256: " + digest + " passes"
    expected = source
    for cut in range(1, len(source)):
        redactor = StreamingRedactor(max_chars=4000)
        emitted = [redactor.push(source[:cut]), redactor.push(source[cut:])]
        assert digest not in "".join(emitted)
        assert "".join(emitted) + redactor.flush(preserve_labeled_digests=True) == expected


def test_overlong_lexical_secret_is_partition_invariant_and_bounded():
    secret = "A1" * 3000
    source = "prefix " + secret + " suffix"
    one, one_pending = _stream_with_metrics([source])
    split, split_pending = _stream_with_metrics([source[i:i + 100] for i in range(0, len(source), 100)])
    assert "".join(one) == "".join(split)
    assert secret not in "".join(one)
    assert "[HIDDEN_SECRET:" in "".join(one)
    assert one_pending <= StreamingRedactor._MAX_PENDING
    assert split_pending <= StreamingRedactor._MAX_PENDING


def test_long_safe_prose_is_partition_invariant_with_bounded_holdback():
    source = "ordinary words " * 390
    one, one_pending = _stream_with_metrics([source])
    split, split_pending = _stream_with_metrics([source[i:i + 100] for i in range(0, len(source), 100)])
    assert "".join(one) == "".join(split) == source
    assert one_pending <= StreamingRedactor._MAX_PENDING
    assert split_pending <= StreamingRedactor._MAX_PENDING


def test_very_large_single_field_does_not_enter_unbounded_holdback():
    source = "prefix " + "a" * 1_000_000
    redactor = StreamingRedactor(max_chars=4000)
    output = redactor.push(source) + redactor.flush()
    assert output == "prefix [REDACTED_STREAM]"
    assert len(redactor._pending) <= StreamingRedactor._MAX_PENDING


def test_redacted_stream_replay_is_exact_and_does_not_reveal_digest_early():
    digest = "1" * 64
    source = "Hash: " + digest + " passes"
    live, pending = _stream_with_metrics(
        [source[:6], source[6:20], source[20:]],
        preserve_labeled_digests=True,
    )
    assert digest not in "".join(live[:-1])
    replayed, _ = _stream_with_metrics(
        ["".join(live)], preserve_labeled_digests=True,
    )
    replayed = "".join(replayed)
    assert replayed == source
    assert pending <= StreamingRedactor._MAX_PENDING


def test_oversized_labeled_digest_with_later_prose_is_partition_invariant():
    digest = "1" * 64
    source = "Hash: " + digest + " passes" + (" word" * 1000)
    one, one_pending = _stream_with_metrics(
        [source], preserve_labeled_digests=True,
    )
    split, split_pending = _stream_with_metrics(
        [source[i:i + 100] for i in range(0, len(source), 100)],
        preserve_labeled_digests=True,
    )
    assert "".join(one) == "".join(split)
    assert digest not in "".join(one)
    assert one_pending <= StreamingRedactor._MAX_PENDING
    assert split_pending <= StreamingRedactor._MAX_PENDING


# -- Streaming partition invariance for the
# authorization-scheme, assignment, claim and known-token rules. The
# streaming result for one chunk, char-by-char, and every two-way cut must
# equal the whole-text scrub_text/scrub_display_text result, with
# preserve_paths both False and True.

_PARTITION_CASES = [
    "Authorization: token dummy123456",
    "authorization: Token dummy123456 rejected",
    "Proxy-Authorization: token dummy123456",
    "Authorization: Basic ZHVtbXk6ZHVtbXlwYXNz",
    "Proxy-Authorization: Basic ZHVtbXk6ZHVtbXlwYXNz",
    "Authorization: Bearer dummy123456",
    'Authorization: Digest username="u", realm="r", response="dummy123456"',
    "Authorization: Custom dummy123456",
    "Authorization : token dummy123456",
    "Authorization=token dummy123456",
    '-H "Authorization: token dummy123456" -d x',
    "line1\nAuthorization: token dummy123456\nline3 token handling",
    '{"Authorization": "token dummy123456", "x": 1}',
    "{'authorization': 'token dummy123456'}",
    '{"token": "dummy123456"}',
    "token dummy123456",
    "Token dummy123456",
    "token=dummy123456",
    "token: dummy123456",
    "input_tokens: dummy123456",
    "token_value: dummy123456",
    "password_hash: dummy123456",
    "the token is dummy123456",
    "the secret was dummy123456",
    "ghp_" + "a1b2c3" * 4,
    "github_pat_" + "a1b2c3" * 4,
    "glpat-" + "a1b2c3" * 4,
    "xoxb-1234567890",
    "sk-ant-api03-" + "a1b2c3" * 4,
    "AKIA" + "0" * 16,
    "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
    "token " + "ghp_" + "a1b2c3" * 4,
    "sk-learn-compatibility-layer-for-blocks",
    "token handling",
    "the token budget is 4096 per turn",
    "Authorization header missing",
    "authorization required",
    "includes/acf-blocks-v2-iframe-compat.php",
]


def _run_streamed(parts, *, preserve_paths):
    redactor = StreamingRedactor(max_chars=4000, preserve_paths=preserve_paths)
    output = "".join(redactor.push(part) for part in parts)
    output += redactor.flush()
    return output


@pytest.mark.parametrize("preserve_paths", [False, True])
@pytest.mark.parametrize("source", _PARTITION_CASES)
def test_t7_streaming_partition_invariance_for_h2_cases(source, preserve_paths):
    scrub = scrub_display_text if preserve_paths else scrub_text
    expected = scrub(source, max_chars=4000)
    assert _run_streamed([source], preserve_paths=preserve_paths) == expected
    assert _run_streamed(list(source), preserve_paths=preserve_paths) == expected
    for cut in range(len(source) + 1):
        assert _run_streamed([source[:cut], source[cut:]], preserve_paths=preserve_paths) == expected
