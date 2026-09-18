"""No hidden native/legacy Codex fallback from any cortex_runtime/engine module.

F4 (T013 review): this boundary is not scoped to T013's own two new
modules -- it is checked here for **every** ``cortex_runtime/engine/*.py``
module (globbed, so a module added by a later task is automatically
covered on its next run; see ``docs/acp/codex-acp-adapter.md``'s "No
native fallback" section for the scope note this test backs). No module
under ``engine/`` may import the legacy native Codex ``_meta`` adapter
(``cortex_runtime.codex_adapter``), ``server.py``, ``hooks.py``,
``host_source.py``, ``marketplace_bootstrap.py``, or reference anything
under a ``.codex-plugin`` path in a code string literal -- and a
``ConfigurationRejected``/engine ``PortableError`` raised anywhere in
``engine/`` must never itself construct or reference any of those either.
This is checked both by AST import inspection (never executes the legacy
modules, only parses source) and by scanning code string literals
(docstrings/comments are exempt, since documenting what is *not*
referenced is expected).
"""
from __future__ import annotations

import ast
from pathlib import Path

import pytest

ENGINE_DIR = Path(__file__).resolve().parents[1] / "src/cortex_runtime/engine"
FORBIDDEN_MODULE_SUBSTRINGS = (
    "codex_adapter",  # the *legacy* module name (this task's own module is codex_adapter_acp / codex_install)
    "server",
    "hooks",
    "host_source",
    "marketplace_bootstrap",
)
FORBIDDEN_PATH_TOKENS = (".codex-plugin",)


def _module_files():
    # Every cortex_runtime/engine/*.py module, not only this task's own two
    # (F4, T013 review) -- a later task adding a new module here is
    # automatically covered by this glob on its next test run.
    return sorted(ENGINE_DIR.glob("*.py"))


def _imported_module_names(tree: ast.Module) -> list[str]:
    names = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            names.extend(alias.name for alias in node.names)
        elif isinstance(node, ast.ImportFrom):
            if node.module:
                names.append(node.module)
    return names


@pytest.mark.parametrize("path", _module_files(), ids=lambda path: path.name)
def test_no_forbidden_module_imports(path):
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    imported = _imported_module_names(tree)
    hits = []
    for name in imported:
        # "codex_adapter_acp" itself and "codex_install" are fine (this
        # task's own modules); only the *legacy* "codex_adapter" name (an
        # exact trailing component, never a substring match against this
        # module's own longer name) is forbidden.
        last_component = name.rsplit(".", 1)[-1]
        if last_component == "codex_adapter":
            hits.append(name)
            continue
        for token in ("server", "hooks", "host_source", "marketplace_bootstrap"):
            if last_component == token:
                hits.append(name)
    assert not hits, f"{path.name} imports forbidden legacy module(s): {hits}"


def _string_literals(tree: ast.Module) -> list[str]:
    # Only actual string *literals* used as code (e.g. a path built into a
    # command or compared against) -- never prose in a docstring/comment,
    # which may legitimately name a forbidden path to document that it is
    # *not* referenced.
    values = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            values.append(node.value)
    return values


@pytest.mark.parametrize("path", _module_files(), ids=lambda path: path.name)
def test_no_forbidden_path_tokens_in_string_literals(path):
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    docstrings = set()
    for node in ast.walk(tree):
        if isinstance(node, (ast.Module, ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            doc = ast.get_docstring(node, clean=False)
            if doc is not None:
                docstrings.add(doc)
    hits = []
    for literal in _string_literals(tree):
        if literal in docstrings:
            continue
        for token in FORBIDDEN_PATH_TOKENS:
            if token in literal:
                hits.append((token, literal))
    assert not hits, f"{path.name} has forbidden legacy path token(s) in a code string literal: {hits}"


def test_configuration_rejected_import_is_the_portable_one_not_legacy():
    """``ConfigurationRejected`` is imported from ``configuration.service``
    (the portable configuration layer), never from the legacy adapter."""
    tree = ast.parse((ENGINE_DIR / "codex_adapter_acp.py").read_text(encoding="utf-8"))
    import_froms = [node for node in ast.walk(tree) if isinstance(node, ast.ImportFrom)]
    matches = [node for node in import_froms if node.module and node.module.endswith("configuration.service")]
    assert matches, "expected an explicit 'from ...configuration.service import ConfigurationRejected'"
    names = {alias.name for node in matches for alias in node.names}
    assert "ConfigurationRejected" in names


def test_engine_dir_modules_never_build_a_dot_codex_plugin_path_at_runtime():
    for path in _module_files():
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        docstrings = set()
        for node in ast.walk(tree):
            if isinstance(node, (ast.Module, ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                doc = ast.get_docstring(node, clean=False)
                if doc is not None:
                    docstrings.add(doc)
        for literal in _string_literals(tree):
            if literal in docstrings:
                continue
            assert ".codex-plugin" not in literal, f"{path.name} builds a .codex-plugin path at runtime: {literal!r}"
