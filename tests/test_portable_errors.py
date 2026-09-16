import pytest

from cortex_runtime.portable.errors import (
    PortableError,
    PortableErrorCategory,
    STORE_ERROR_CODE_MAPPING,
)
from cortex_runtime.portable.version import CONTRACT_VERSION


def test_all_error_categories_are_represented():
    assert {member.value for member in PortableErrorCategory} == {
        "validation", "authorization", "transport", "engine_failure", "indeterminate_write",
    }


def test_portable_error_requires_explicit_retry_eligible():
    with pytest.raises(TypeError):
        PortableError(category=PortableErrorCategory.VALIDATION, code="x", remediation="fix it")  # type: ignore[call-arg]

    error = PortableError(category=PortableErrorCategory.VALIDATION, code="x", remediation="fix it", retry_eligible=False)
    assert error.retry_eligible is False
    assert error.category == PortableErrorCategory.VALIDATION
    assert error.code == "x"
    assert error.remediation == "fix it"


def test_portable_error_retry_eligible_must_be_a_bool():
    with pytest.raises(TypeError):
        PortableError(category=PortableErrorCategory.TRANSPORT, code="x", remediation="fix it", retry_eligible="yes")


def test_store_error_code_mapping_covers_representative_codes_with_remediation():
    # A hand-picked subset of existing cortex_runtime.contracts.StoreError
    # codes (documented mapping, not exhaustive; StoreError is storage-only
    # and so never maps to the engine_failure category).
    assert STORE_ERROR_CODE_MAPPING  # non-empty
    for code, (category, remediation, retry_eligible) in STORE_ERROR_CODE_MAPPING.items():
        assert isinstance(code, str) and code
        assert isinstance(category, PortableErrorCategory)
        assert isinstance(remediation, str) and remediation
        assert isinstance(retry_eligible, bool)
    seen_categories = {category for category, _, _ in STORE_ERROR_CODE_MAPPING.values()}
    assert seen_categories == {
        PortableErrorCategory.VALIDATION,
        PortableErrorCategory.AUTHORIZATION,
        PortableErrorCategory.TRANSPORT,
        PortableErrorCategory.INDETERMINATE_WRITE,
    }


def test_every_portable_error_category_supports_explicit_remediation_and_retry_eligible():
    for category in PortableErrorCategory:
        for retry_eligible in (True, False):
            if category == PortableErrorCategory.INDETERMINATE_WRITE and retry_eligible:
                continue  # covered separately: this combination is rejected
            error = PortableError(
                category=category, code=f"{category.value}_example",
                remediation=f"Representative remediation for {category.value}.",
                retry_eligible=retry_eligible,
            )
            assert error.category == category
            assert error.remediation
            assert error.retry_eligible is retry_eligible


def test_indeterminate_write_can_never_be_constructed_retry_eligible():
    # The category invariant: an indeterminate write's outcome is unknown,
    # so it must never claim a blind retry is safe.
    error = PortableError(
        category=PortableErrorCategory.INDETERMINATE_WRITE, code="uncertain_write",
        remediation="Reconcile first, then resend the identical request and delivery key.",
        retry_eligible=False,
    )
    assert error.retry_eligible is False

    with pytest.raises(ValueError):
        PortableError(
            category=PortableErrorCategory.INDETERMINATE_WRITE, code="uncertain_write",
            remediation="Reconcile first, then resend the identical request and delivery key.",
            retry_eligible=True,
        )


def test_store_error_code_mapping_never_marks_indeterminate_write_retry_eligible():
    for code, (category, _remediation, retry_eligible) in STORE_ERROR_CODE_MAPPING.items():
        if category == PortableErrorCategory.INDETERMINATE_WRITE:
            assert retry_eligible is False, f"{code} must not be retry_eligible=True for indeterminate_write"


def test_storage_error_mapping_states_reconciliation_not_blind_retry():
    category, remediation, retry_eligible = STORE_ERROR_CODE_MAPPING["storage_error"]
    assert category == PortableErrorCategory.INDETERMINATE_WRITE
    assert retry_eligible is False
    assert "reconcil" in remediation.lower()


def test_portable_error_raisable_and_catchable_as_exception():
    def raise_it():
        raise PortableError(
            category=PortableErrorCategory.ENGINE_FAILURE,
            code="engine_crashed",
            remediation="Restart the engine session and retry with a fresh session id.",
            retry_eligible=True,
        )

    with pytest.raises(PortableError) as excinfo:
        raise_it()
    assert excinfo.value.category == PortableErrorCategory.ENGINE_FAILURE
    assert excinfo.value.retry_eligible is True
    assert isinstance(excinfo.value, Exception)


def test_contract_version_present_and_is_stable_int():
    assert isinstance(CONTRACT_VERSION, int)
    assert CONTRACT_VERSION == 1
