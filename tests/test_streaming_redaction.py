"""No-paid deterministic coverage for chunk-boundary redaction."""

from hashlib import sha256

from cortex_runtime.observability.redact import StreamingRedactor, scrub_text


def _stream(parts):
    redactor = StreamingRedactor(max_chars=4000)
    emitted = [redactor.push(part) for part in parts]
    emitted.append(redactor.flush())
    return emitted


def test_split_opaque_token_is_never_emitted_before_completion():
    secret = "abcde12345fghij67890klmno"
    emitted = _stream(["ordinary ", secret[:15], secret[15:], " tail"])
    assert emitted[1] == ""
    joined = "".join(emitted)
    assert secret not in joined
    assert joined == scrub_text("ordinary " + secret + " tail", max_chars=4000)


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
