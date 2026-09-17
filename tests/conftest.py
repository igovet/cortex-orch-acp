from pathlib import Path
import sys
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'plugins/cortex/scripts'))
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'scripts'))

import pytest

# --- T009 incident guard -----------------------------------------------
#
# An early draft of a T009 ACP transport test passed the real repository
# checkout (via os.getcwd()) as an ACP session's project root. Opening a
# Store against that path triggered the one-way legacy
# .codex/cortex -> .cortex migration on the real project (see
# project_storage.migrate_legacy_project_storage). This guard exists so
# that mistake, or anything like it, fails the test run immediately
# instead of silently mutating the real checkout again.
#
# It monkeypatches the small set of functions that are the only entry
# points into project-root-scoped storage (canonical_project,
# project_store_directory, migrate_legacy_project_storage,
# logical_project_root) to raise AssertionError whenever the resolved
# path is this repository's own root or anything inside it. tmp_path
# fixtures and isolated scratch clones (anywhere outside this checkout)
# are unaffected.
#
# This only covers code running *inside this pytest process*. It cannot
# reach into a subprocess (the ACP agent is normally exercised as a real
# subprocess -- see tests/acp_fakes.py); subprocess-based ACP tests are
# covered separately by a static source scan (see
# test_acp_transport.py::test_no_test_source_uses_repo_root_as_project_root)
# that flags os.getcwd()/Path.cwd()/ROOT used in a project-root position
# in test source, which is what would have caught the original mistake.
# This module intentionally adds no test-only hook to production code
# (no env var project_storage would have to honor).

REPO_ROOT = Path(__file__).resolve().parents[1]

_GUARDED_NAMES = (
    "canonical_project",
    "project_store_directory",
    "migrate_legacy_project_storage",
    "logical_project_root",
)


def _is_forbidden(value):
    try:
        candidate = Path(value).resolve()
    except (OSError, RuntimeError, TypeError, ValueError):
        return False
    return candidate == REPO_ROOT or REPO_ROOT in candidate.parents


def _guard_value(value, function_name):
    if _is_forbidden(value):
        raise AssertionError(
            "test guard (tests/conftest.py): refusing to let "
            f"{function_name}({value!r}) touch this repository's own "
            f"checkout ({REPO_ROOT}) as a project root. Use tmp_path or an "
            "isolated scratch clone outside the checkout instead."
        )


def _wrap(function_name, original):
    def guarded(value, *args, **kwargs):
        _guard_value(value, function_name)
        return original(value, *args, **kwargs)
    guarded.__cortex_test_guard_original__ = original
    return guarded


def _install_project_root_guard():
    """Patch every currently-bound reference to the guarded functions.

    Patches ``cortex_runtime.project_storage`` itself (the source of
    truth: every other in-package caller resolves these names against
    that module's own globals at call time, including lazy
    ``from .project_storage import ...`` imports inside functions like
    ``store.Store.__init__``, so patching it alone already covers those).
    It also re-points any *already-imported* module that captured its own
    top-level ``from ... import <name>`` reference before this fixture
    ran (for example ``cortex_runtime.server``), since that binding is
    otherwise independent of the source module's attribute.
    """
    import cortex_runtime.project_storage as project_storage_module

    originals = {name: getattr(project_storage_module, name) for name in _GUARDED_NAMES}
    guarded = {name: _wrap(name, original) for name, original in originals.items()}
    patched = []

    for name, replacement in guarded.items():
        setattr(project_storage_module, name, replacement)
        patched.append((project_storage_module, name, originals[name]))

    for module in list(sys.modules.values()):
        if module is None or module is project_storage_module:
            continue
        for name, original in originals.items():
            try:
                if getattr(module, name, None) is original:
                    setattr(module, name, guarded[name])
                    patched.append((module, name, original))
            except Exception:
                continue
    return patched


@pytest.fixture(autouse=True, scope="session")
def _forbid_real_checkout_as_project_root():
    patched = _install_project_root_guard()
    try:
        yield
    finally:
        for module, name, original in patched:
            setattr(module, name, original)
