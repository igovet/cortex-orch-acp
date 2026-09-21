"""PermissionBridge: relay an engine's ``session/request_permission`` to the
host, bound to engine/session/action/workspace/ownership-epoch identity, and
never relay a stale decision (T014).

Every reply this bridge produces to the *engine* is schema-valid ACP:
either ``{"outcome": {"outcome": "cancelled"}}`` or ``{"outcome":
{"outcome": "selected", "optionId": <one of the engine's own offered
optionIds>}}`` -- never any other shape, and never an ``optionId`` the
engine itself did not offer in this same request. ``allow_always``/
``reject_always`` are deliberately treated as an ordinary single-action
``selected``/``cancelled`` decision: this bridge never writes or consults
any durable "standing rule" state, so no host "always" choice is ever
silently reused for a later, different action.

T035 ("workers get full access except deletions", user requirement):
a **worker** binding (``ResourceBinding.role == "worker"``) whose request
:func:`requires_confirmation` classifies as *not* a deletion is answered
locally with that request's own offered allow option, without ever reaching
the host -- so a worker's ordinary operations are never slowed down by a
confirmation prompt. Claude ACP may expose only ``allow_always`` for this
request shape; selecting that offered option is still a local per-request
Cortex decision, and Cortex stores no standing permission rule.

T035 FINDING-12 closure ("the coordinator must not be able to work on the
project" -- see ``.cortex/acp-migration/evidence/T035/FINDING-12-read-only-
mode-does-not-prevent-edits.md`` and ``configuration.policy
.COORDINATOR_MODE_FLOOR``'s own docstring): a **coordinator** binding
(``ResourceBinding.role == "coordinator"``) is never auto-approved for a
mutation. Live evidence proved the previously-chosen mechanism
(the engine's own non-editing mode) does not stop a coordinator from
editing the project, and additionally hides the attempt from this bridge
entirely (that mode never issues ``session/request_permission`` for the
edits it silently performs). With the coordinator floor now on the same
mode the worker floor already uses, edit attempts DO reach this bridge --
and every one :func:`is_coordinator_mutating` classifies as mutating is
refused by policy first. Coordinator source edits are hard local refusals;
other reviewable actions are shown to the connected user as a fresh one-shot
request when a host requester exists. Missing, invalid, stale, or denied
decisions still refuse them; worker writes outside their bound workspace remain
a hard deny. The one local exception is an exact store-backed edit of its own
active pipeline draft; every
non-mutating (read/search/fetch/...) request is auto-approved right here
instead, so a coordinator can still inspect the repository to decide what
to delegate. The policy refusal is Cortex-owned, while the connected user
owns the final one-shot decision for an otherwise actionable request.

HONEST BOUNDARY: this only ever inspects the ACP ``toolCall`` payload the
engine itself chose to declare (``kind``/``rawInput``/``title``) on a
``session/request_permission`` call the engine actually issued. A native
tool that deletes a file without ever issuing that request (see
``engine/fs_bridge.py``, ``terminal_bridge.py``, ``resource_policy.py``'s
own docstrings on the same limit) is invisible here and is not, and
cannot be, caught by this bridge. See
``delegation.revision``/``acp.orchestrating_runtime``'s coordinator-turn
workspace-digest comparison for the engine-independent backstop this gap
is exactly why that backstop exists: detection after the fact, not
prevention, for whatever a coordinator's engine executes without ever
asking this bridge at all.
"""
from __future__ import annotations

import ast
from collections.abc import Mapping
from pathlib import Path
import re
import shlex
import threading

CANCELLED_OUTCOME = {"outcome": {"outcome": "cancelled"}}

# ToolKind values (ACP schema-1.4.0's $defs/ToolKind) that unambiguously
# remove or replace a file/target. "move" is included because a rename can
# silently destroy an existing file at the destination path.
_DELETION_TOOL_KINDS = frozenset({"delete", "move"})

# ToolKind values that are never a deletion under the ACP spec's own
# definition, and get no further command-text scan.
_NON_DELETION_TOOL_KINDS = frozenset({"read", "edit", "search", "fetch", "think", "switch_mode", "execute"})

# Deliberately conservative shell idioms that remove or irreversibly
# truncate file content: bare rm/rmdir/unlink/shred, git clean, `find
# -delete`, mv (can overwrite/destroy a target), and a truncating '>'
# redirect (never '>>', which only appends). This is pattern-matching over
# a command string -- inherently incomplete (see module docstring) -- so it
# is only ever used to *upgrade* an "execute" tool call to "ask", never to
# downgrade a "delete"/"move" kind the engine already declared.
_DELETION_COMMAND_PATTERN = re.compile(
    r"(?:^|[;&|]\s*)(?:sudo\s+)?(?:(?:/usr/bin/|/bin/)?(?:rm|rmdir|unlink|shred|mv)|git\s+clean|find\b[^;&|]*-delete)\b"
    r"|(?<!>)>(?!>)"
    r"|\bdelet(?:e|ing|ed)\b",
    re.IGNORECASE,
)

# Everything above except the truncating redirect, which is handled
# separately: see _redirects_only_into_the_store.
_DELETION_COMMAND_WITHOUT_REDIRECT = re.compile(
    r"(?:^|[;&|]\s*)(?:sudo\s+)?(?:(?:/usr/bin/|/bin/)?(?:rm|rmdir|unlink|shred|mv)|git\s+clean|find\b[^;&|]*-delete)\b"
    r"|\bdelet(?:e|ing|ed)\b",
    re.IGNORECASE,
)

_RM_COMMAND = re.compile(r"(?:^|[;&|]\s*)(?:sudo\s+)?(?:/usr/bin/|/bin/)?rm\b", re.I)
_DIRECT_DESTRUCTIVE_WORD = re.compile(r"\b(?:rm|rmdir|unlink|shred|mv)\b", re.I)

# Directly observable destructive APIs in an otherwise opaque interpreter
# body.  This is intentionally limited to literal API spellings and literal
# write modes; dynamic dispatch and computed arguments remain outside the
# lexical ACP boundary and are documented as such.
_PYTHON_DESTRUCTIVE_API = re.compile(
    r"\bos\s*\.\s*(?:remove|unlink|rmdir)\s*\(|"
    r"\bshutil\s*\.\s*rmtree\s*\(|"
    r"(?:\bPath\s*\([^\n]*?\)|\b[A-Za-z_]\w*)\s*\.\s*(?:unlink|rmdir)\s*\(|"
    r"\bsubprocess\s*\.\s*(?:run|call|check_call|check_output|Popen)\s*\([^\n]*(?:['\"])(?:/[^'\"]*/)?(?:rm|rmdir|unlink|shred)(?:['\"]),|"
    r"\bopen\s*\([^\n]*,\s*['\"][wax]['\"]",
    re.IGNORECASE,
)


def is_rm_command(command: str) -> bool:
    return bool(_RM_COMMAND.search(command))


def _contains_observable_destructive_api(command: str) -> bool:
    """Detect literal deletion/truncation APIs inside an interpreter body.

    This supplements the shell detector for Claude's ``kind=other`` Bash
    payloads. It is not a Python parser and makes no claim about dynamic code.
    """
    return bool(_PYTHON_DESTRUCTIVE_API.search(command or ""))

# A truncating redirect and the path it writes to.
_REDIRECT_TARGET = re.compile(r"(?<!>)>(?!>)\s*'?\"?([^\s'\";&|]+)")


def _redirects_only_where_nothing_can_be_lost(text: str) -> bool:
    """True when every truncating redirect in ``text`` writes somewhere
    nothing can be destroyed, and there is at least one.

    Two such places: Cortex's own store (``.cortex/``) and ``/dev/null``.
    The second matters more than it looks -- ``2>/dev/null`` is on the end
    of half the commands anyone writes, and it made a read-only ``find``
    look like a deletion and stop to ask the user.

    A worker fills its report by writing the draft file the runtime handed
    it, and a shell heredoc does that with ``>``. Treating every ``>`` as
    a deletion sent that to the user for approval as an unexplained "Bash"
    prompt offering to "allow access to draft-reports/" -- a question
    about Cortex's own bookkeeping that the user has no way to judge and
    should never have been asked.

    Deliberately narrow: only the redirect is reconsidered. ``rm``,
    ``shred``, ``git clean`` and the rest still count as deletions
    wherever they point, including inside the store.
    """
    targets = _REDIRECT_TARGET.findall(text)
    if not targets:
        return False
    return all(
        _CORTEX_STORE_PATH.search(target) or target in _DISCARD_TARGETS
        for target in targets
    )


def _redirects_only_inside_worker_workspace(text: str, workspace_root: str) -> bool:
    """Prove shell redirects stay in the worker's source workspace.

    A normal redirect is an in-scope write, not a deletion policy event. The
    target must resolve below the bound workspace and may not enter Cortex's
    private store. This helper only handles redirects; command classification
    still rejects the known destructive shell/API forms separately.
    """
    targets = _REDIRECT_TARGET.findall(text)
    if not targets or not isinstance(workspace_root, str):
        return False
    try:
        root = Path(workspace_root).resolve(strict=True)
    except (OSError, RuntimeError, ValueError):
        return False
    for target in targets:
        if target in _DISCARD_TARGETS:
            continue
        try:
            candidate = Path(target) if Path(target).is_absolute() else root / target
            resolved = candidate.resolve(strict=False)
            resolved.relative_to(root)
        except (OSError, RuntimeError, ValueError):
            return False
        if ".cortex" in resolved.relative_to(root).parts:
            return False
    return True


# Writing here destroys nothing.
_DISCARD_TARGETS = frozenset({"/dev/null", "/dev/stdout", "/dev/stderr"})


# The store's own directory as it appears in a path, absolute or relative.
_CORTEX_STORE_PATH = re.compile(r"(?:^|/)\.cortex/")


# How much of a command a permission card can carry before it stops being
# a question and becomes a wall of text.
_PERMISSION_COMMAND_MAX_CHARS = 400

_PERMISSION_SENSITIVE = re.compile(
    r"(?i)(?:api[_-]?key|token|password|passwd|secret|authorization|bearer)\b"
    r"|(?:https?://[^\s]+[?&](?:token|key|secret|password|auth)=)"
    r"|https?://[^\s/@]+:[^\s/@]+@"
    r"|[{'\"](?:token|api[_-]?key|password|secret|authorization)[{'\"]?\s*:"
)
_PERMISSION_SHELL_OPERATOR = re.compile(r"(?:^|[^\\])[;&|<>`]" )
_PERMISSION_SAFE_EXECUTABLE = re.compile(r"[A-Za-z][A-Za-z0-9_.+-]*\Z")


def _safe_permission_command_summary(command: str) -> str:
    """Return a conservative, credential-free command summary.

    Permission cards are user-facing. If quoting, shell composition, or a
    sensitive marker prevents a proof that every argument is harmless, keep
    only the executable name and hide the complete argument span.
    """
    normalized = " ".join(command.split())
    if not normalized:
        return ""
    try:
        words = shlex.split(normalized, posix=True)
    except ValueError:
        # A malformed quote makes token boundaries unprovable. Do not reuse
        # even the apparent first token: it may itself be a secret.
        return "arguments hidden"
    if not words:
        return ""
    # An assignment prefix can carry a secret and is not a command name. Do
    # not skip it to find a later executable: ``TOKEN=... curl`` must remain
    # entirely generic so the assignment cannot leak into the card.
    executable = words[0]
    if not _PERMISSION_SAFE_EXECUTABLE.fullmatch(executable) or "=" in executable:
        return "arguments hidden"
    if len(words) == 1:
        return executable
    if _PERMISSION_SENSITIVE.search(normalized) or _PERMISSION_SHELL_OPERATOR.search(normalized):
        return f"{executable} — arguments hidden"
    return " ".join(words)


def _named_for_the_user(params):
    """The same request, with the command it is actually asking about put
    where the user will see it.

    An engine names a shell call after its tool ("Bash"), and the command
    itself sits in ``rawInput`` -- which a client renders, at best, behind
    a disclosure. What reached the user was a prompt reading "Run Command
    / Bash" with a Yes/No: a decision with the one fact needed to make it
    missing. This only ever *adds* to the title; nothing else about the
    request is altered, and a request that already names its command is
    left alone.
    """
    tool_call = _tool_call(params)
    if not tool_call:
        return params
    command = " ".join(_command_text_only(tool_call).split())
    kind = tool_call.get("kind")
    if command:
        command = _safe_permission_command_summary(command)
        if len(command) > _PERMISSION_COMMAND_MAX_CHARS:
            command = command[: _PERMISSION_COMMAND_MAX_CHARS - 1] + "\u2026"
        # The engine-controlled title is untrusted and may contain a secret
        # even when rawInput was sanitized. Never copy it into the card.
        title = f"Run command: {command}"
    elif kind == "delete":
        title = "Delete request"
    elif kind == "move":
        title = "Move request"
    elif kind == "edit":
        title = "Edit request"
    elif kind == "read":
        title = "Read request"
    elif kind == "search":
        title = "Search request"
    else:
        # A title-only request cannot be safely summarized from untrusted
        # model text. Keep the action class and discard the title entirely.
        title = "Requested action"
    named = dict(tool_call)
    named["title"] = title
    updated = dict(params)
    updated["toolCall"] = named
    return updated


# A worker's own draft/pipeline file lives in the project's store, outside
# its isolated workspace, and filling it is exactly what a worker is for.
_WORKER_WRITABLE_STORE_FILE = re.compile(r"/\.cortex/(?:draft-reports|pipeline-drafts)/[^/]+$")

# The kinds that change something. "execute" is deliberately not here --
# see worker_write_outside_workspace's own HONEST BOUNDARY.
_MUTATING_TOOL_KINDS = frozenset({"edit", "delete", "move"})

# Where an engine puts the path a tool call acts on, besides `locations`.
_TOOL_PATH_KEYS = ("path", "file_path", "filePath", "abs_path", "absolutePath", "target", "destination")


def worker_write_outside_workspace(params, workspace_root: str) -> str | None:
    """The first path this call would change that lies outside the worker's
    own isolated workspace, or ``None`` when it changes nothing outside it.

    A worker works on a copy, and its result reaches the project only
    through the conflict-checked integration step. That was true only by
    convention: the copy was the worker's working directory, and nothing
    stopped a worker that knew the project's real path from editing it
    directly. Live, one did -- its `Edit` named the real
    ``/var/tmp/.../calc.py``, its own copy stayed untouched, there was no
    integration step at all, and the coordinator accepted a change that had
    never been checked against anything.

    A worker's own draft/pipeline file is the one thing it may write
    outside that copy: it lives in the project's store by design, and
    filling it is what a worker is for.

    HONEST BOUNDARY: this reads the paths a tool call declares -- its
    ``locations`` and the usual ``rawInput`` path keys. A shell command can
    still write wherever the user's own account can; that vector is bounded
    by ``requires_confirmation`` (which sends destructive shell idioms to
    the user) and not by this. Closing the declared-path vector is what
    stops the ordinary case, which is an edit tool pointed at an absolute
    path.
    """
    tool_call = _tool_call(params)
    if tool_call.get("kind") not in _MUTATING_TOOL_KINDS:
        return None
    try:
        root = Path(workspace_root).resolve()
    except OSError:
        return None
    candidates = []
    for location in tool_call.get("locations") or ():
        if isinstance(location, Mapping) and isinstance(location.get("path"), str):
            candidates.append(location["path"])
    raw_input = tool_call.get("rawInput")
    if isinstance(raw_input, Mapping):
        candidates.extend(raw_input[key] for key in _TOOL_PATH_KEYS if isinstance(raw_input.get(key), str))
    for candidate in candidates:
        if not candidate.startswith("/"):
            # Relative to the worker's own working directory, which is the
            # workspace itself.
            continue
        if _WORKER_WRITABLE_STORE_FILE.search(candidate):
            continue
        try:
            resolved = Path(candidate).resolve()
        except OSError:
            return candidate
        if resolved != root and root not in resolved.parents:
            return candidate
    return None


def _coordinator_edit_in_workspace(params, workspace_root: str | None) -> bool:
    """Return true only for a declared edit fully inside project source."""
    if _tool_call(params).get("kind") != "edit" or not isinstance(workspace_root, str):
        return False
    try:
        root = Path(workspace_root).resolve(strict=True)
    except (OSError, RuntimeError, ValueError):
        return False
    candidates: list[str] = []
    tool_call = _tool_call(params)
    for location in tool_call.get("locations") or ():
        if isinstance(location, Mapping) and isinstance(location.get("path"), str):
            candidates.append(location["path"])
    raw_input = tool_call.get("rawInput")
    if isinstance(raw_input, Mapping):
        candidates.extend(raw_input[key] for key in _TOOL_PATH_KEYS if isinstance(raw_input.get(key), str))
    if not candidates:
        return False
    for candidate in candidates:
        try:
            resolved = (Path(candidate) if Path(candidate).is_absolute() else root / candidate).resolve(strict=False)
            resolved.relative_to(root)
        except (OSError, RuntimeError, ValueError):
            return False
        if ".cortex" in resolved.relative_to(root).parts:
            return False
    return True


def _declared_path_outside_workspace(params, workspace_root: str | None) -> str | None:
    """Return a declared path outside the bound non-private workspace."""
    if not isinstance(workspace_root, str):
        return None
    try:
        root = Path(workspace_root).resolve(strict=True)
    except (OSError, RuntimeError, ValueError):
        return None
    tool_call = _tool_call(params)
    candidates: list[str] = []
    for location in tool_call.get("locations") or ():
        if isinstance(location, Mapping) and isinstance(location.get("path"), str):
            candidates.append(location["path"])
    raw_input = tool_call.get("rawInput")
    if isinstance(raw_input, Mapping):
        candidates.extend(raw_input[key] for key in _TOOL_PATH_KEYS if isinstance(raw_input.get(key), str))
    # Claude's Bash ACP shape commonly carries the target only inside the
    # command string (for example ``find /``), with no structured path field.
    # Include absolute command operands so an outside read reaches the same
    # user-confirmation boundary as a structured outside path. Executable
    # locations and standard device paths are not targets.
    if tool_call.get("kind") in {"other", "execute"}:
        command = _command_text_only(tool_call)
        try:
            # Do not parse interpreter bodies as shell operands: a Python
            # heredoc can contain division tokens and absolute-looking
            # strings unrelated to the command's filesystem target.
            candidates.extend(shlex.split(command.split("<<", 1)[0], posix=True))
        except ValueError:
            return "unparseable command"
    system_prefixes = ("/bin/", "/usr/bin/", "/usr/local/bin/", "/dev/")
    for candidate in candidates:
        if isinstance(candidate, str) and candidate.startswith(system_prefixes):
            continue
        try:
            resolved = (Path(candidate) if Path(candidate).is_absolute() else root / candidate).resolve(strict=False)
            resolved.relative_to(root)
        except (OSError, RuntimeError, ValueError):
            return candidate
        if ".cortex" in resolved.relative_to(root).parts:
            return candidate
    return None


def _coordinator_private_or_outside_access(params, workspace_root: str | None) -> str | None:
    """Return a coordinator path that must be refused locally.

    The coordinator may inspect and test the opened project, but it must not
    turn a non-destructive read/write aimed at another directory into a host
    permission question.  Deletions are intentionally excluded by the
    caller so the user's only approval surface remains an exact destructive
    action.
    """
    if not isinstance(workspace_root, str):
        return None
    root = Path(workspace_root).resolve(strict=True)
    tool_call = _tool_call(params)
    candidates: list[str] = []
    for location in tool_call.get("locations") or ():
        if isinstance(location, Mapping) and isinstance(location.get("path"), str):
            candidates.append(location["path"])
    raw_input = tool_call.get("rawInput")
    if isinstance(raw_input, Mapping):
        candidates.extend(raw_input[key] for key in _TOOL_PATH_KEYS if isinstance(raw_input.get(key), str))
    command = _command_text_only(tool_call) if tool_call.get("kind") in {"other", "execute"} else ""
    if command:
        # Keep command tokens deliberately conservative.  System launchers
        # are not targets; any other absolute path, traversal, or .cortex
        # reference is outside the coordinator's project boundary.
        try:
            candidates.extend(shlex.split(command, posix=True))
        except ValueError:
            return "unparseable command"
        candidates.extend(re.findall(r"(?<![A-Za-z0-9_.-])/(?:[^\s'\";&|<>`]+)", command))
    system_prefixes = ("/bin/", "/usr/bin/", "/usr/local/bin/", "/dev/")
    for candidate in candidates:
        if not isinstance(candidate, str):
            continue
        if ".cortex" in candidate.split("/") or candidate == ".cortex" or candidate.startswith(".cortex/"):
            return candidate
        if candidate == ".." or candidate.startswith("../") or "/../" in candidate:
            return candidate
        if not candidate.startswith("/"):
            continue
        if candidate.startswith(system_prefixes) or re.fullmatch(r"/tmp/coord-[A-Za-z0-9_.-]+(?:/.*)?", candidate):
            continue
        try:
            Path(candidate).resolve(strict=False).relative_to(root)
        except (OSError, RuntimeError, ValueError):
            return candidate
    return None


def _command_text_only(tool_call: dict) -> str:
    """The command as the engine declared it -- never the title, which is
    what :func:`_command_text` falls back to for classification."""
    raw_input = tool_call.get("rawInput")
    if not isinstance(raw_input, dict):
        return ""
    parts = [raw_input[key] for key in ("command", "cmd", "script") if isinstance(raw_input.get(key), str)]
    return "\n".join(parts)


# Some engines (notably Claude ACP builds) describe a shell request as ACP
# ``kind=other`` instead of ``kind=execute``. An opaque ``other`` request
# must still fail closed, but refusing a clearly read-only command such as
# ``cat README && echo --- && git status --short`` would be a false refusal.
# This is intentionally a tiny grammar, not a shell parser. It covers only
# the exact observational forms we can prove here; the fallback must never
# become a general shell allowlist.
_READ_ONLY_SHELL_UNSAFE = re.compile(r"(?:[|;<>`'\"\\$*?\[\]{}()]|\n|\r)")
_SAFE_RELATIVE_READ_PATH = re.compile(r"[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*\Z")

_SAFE_OTHER_WORD = re.compile(r"[A-Za-z0-9_./+=:-]+\Z")
_SAFE_PYTEST_FLAGS = frozenset({"-q", "-v", "-vv", "-x"})

# Claude's ACP adapter currently sends some Bash requests as ``kind=other``.
# A heredoc is shell code, so it must not be admitted merely because the body
# happens to contain the word "python".  This visitor proves a deliberately
# small, pure-Python expression language instead.  In particular, there are
# no imports, attribute escapes, file/process/network APIs, comprehensions,
# generators, or user-defined class objects in the accepted language.
_PURE_PYTHON_CALLS = frozenset({
    "abs", "all", "any", "bool", "dict", "float", "int", "len", "max",
    "min", "pow", "print", "range", "round", "sorted", "str", "sum", "tuple",
})
_PURE_PYTHON_METHODS = frozenset({
    "capitalize", "casefold", "count", "endswith", "find", "format", "join",
    "lower", "replace", "split", "startswith", "strip", "title", "upper",
})


class _ReadOnlyPython(ast.NodeVisitor):
    """Conservative proof that a heredoc cannot perform an external effect."""

    _allowed = frozenset({
        ast.Module, ast.Expr, ast.Assign, ast.AnnAssign, ast.AugAssign,
        ast.Name, ast.Load, ast.Store, ast.Constant, ast.List, ast.Tuple,
        ast.Dict, ast.Set, ast.Subscript, ast.Slice, ast.BinOp, ast.UnaryOp,
        ast.BoolOp, ast.Compare, ast.If, ast.IfExp,
        ast.Call, ast.keyword, ast.arguments, ast.arg, ast.FunctionDef, ast.Return,
        ast.Pass, ast.Break, ast.Continue,
        ast.Add, ast.Sub, ast.Mult, ast.Div, ast.FloorDiv, ast.Mod, ast.Pow,
        ast.UAdd, ast.USub, ast.Not, ast.Invert, ast.And, ast.Or,
        ast.Eq, ast.NotEq, ast.Lt, ast.LtE, ast.Gt, ast.GtE, ast.In, ast.NotIn,
    })

    def __init__(self):
        self.safe = True
        self._function_depth = 0
        self._functions = set()

    def visit_Module(self, node):
        self._functions.update(
            statement.name for statement in node.body if isinstance(statement, ast.FunctionDef)
        )
        for statement in node.body:
            self.visit(statement)

    def generic_visit(self, node):
        if type(node) not in self._allowed:
            self.safe = False
            return
        super().generic_visit(node)

    def visit_Attribute(self, node):
        # Permit only pure string methods on a recursively safe base.  This
        # supports common color parsing (e.g. ``value.strip().lower()``)
        # without opening arbitrary object introspection or module access.
        if node.attr not in _PURE_PYTHON_METHODS:
            self.safe = False
            return
        self.visit(node.value)

    def visit_Call(self, node):
        if isinstance(node.func, ast.Name):
            if node.func.id not in _PURE_PYTHON_CALLS and node.func.id not in self._functions:
                self.safe = False
                return
        elif isinstance(node.func, ast.Attribute):
            self.visit_Attribute(node.func)
            if not self.safe:
                return
        else:
            self.safe = False
            return
        for arg in node.args:
            self.visit(arg)
        for keyword in node.keywords:
            self.visit(keyword)

    def visit_FunctionDef(self, node):
        if (
            self._function_depth
            or node.decorator_list
            or node.returns
            or node.args.defaults
            or node.args.kw_defaults
            or any(argument.annotation is not None for argument in (*node.args.posonlyargs,
                                                                      *node.args.args,
                                                                      *node.args.kwonlyargs))
        ):
            self.safe = False
            return
        self._function_depth += 1
        self.visit(node.args)
        for statement in node.body:
            self.visit(statement)
        self._function_depth -= 1


_PYTHON_HEREDOC = re.compile(
    # Native Claude Bash requests commonly preserve the harmless interpreter
    # stdin spelling (``python3 -B - <<'PY'``), and may prefix it with the
    # standard no-bytecode environment assignment.  Keep both variants
    # inside this same exact heredoc grammar; do not generalize to arbitrary
    # assignments, options, or shell composition.
    r"\A\s*(?:PYTHONDONTWRITEBYTECODE=1\s+)?(?:/usr/bin/)?python3(?:\s+-B)?(?:\s+-)?\s+<<\s*(['\"]?)([A-Za-z_][A-Za-z0-9_]*)\1\s*\n"
    r"(?P<body>.*?)\n\2\s*\Z",
    re.DOTALL,
)


def _is_read_only_python_heredoc(text: str) -> bool:
    """Return true only for a bounded, pure Python stdin script."""
    if not isinstance(text, str):
        return False
    match = _PYTHON_HEREDOC.fullmatch(text)
    if match is None:
        return False
    body = match.group("body")
    try:
        tree = ast.parse(body, mode="exec")
    except (SyntaxError, ValueError, TypeError):
        return False
    checker = _ReadOnlyPython()
    checker.visit(tree)
    return checker.safe


def _safe_worker_path(root: Path, value: str) -> bool:
    if value.startswith("-") or not value.endswith(".py"):
        return False
    try:
        (root / value).resolve(strict=False).relative_to(root)
    except (OSError, ValueError):
        return False
    return True


def _safe_pytest_args(root: Path, args: list[str]) -> bool:
    for arg in args:
        if arg in _SAFE_PYTEST_FLAGS:
            continue
        if _safe_worker_path(root, arg):
            continue
        return False
    return True


def _safe_python_cli_args(root: Path, args: list[str]) -> bool:
    index = 0
    while index < len(args):
        arg = args[index]
        if arg == "--step":
            if index + 1 >= len(args) or not re.fullmatch(r"-?[0-9]+", args[index + 1]):
                return False
            index += 2
            continue
        if re.fullmatch(r"-?[0-9]+", arg):
            index += 1
            continue
        return False
    return True


def _is_worker_safe_other_command(text: str, workspace_root: str) -> bool:
    """Recognize the narrow read/test Bash forms Claude reports as ``other``."""
    if not isinstance(text, str) or not text.strip() or not isinstance(workspace_root, str):
        return False
    try:
        root = Path(workspace_root).resolve(strict=True)
    except OSError:
        return False
    if _is_read_only_python_heredoc(text):
        return True
    text = text.replace("$?", "STATUS").replace("2>&1", "")
    if _DELETION_COMMAND_PATTERN.search(text):
        return False
    if any(token in text for token in ("&&", "||", "`", "$", "<", ">")):
        return False
    if "\\n" in text or "\\r" in text:
        return False
    segments = [part.strip() for part in re.split(r"[;|]", text) if part.strip()]
    if not segments:
        return False
    for index, segment in enumerate(segments):
        try:
            words = shlex.split(segment, posix=True)
        except ValueError:
            return False
        if not words or any(not _SAFE_OTHER_WORD.fullmatch(word) for word in words):
            return False
        executable = Path(words[0]).name
        if executable == "python3":
            if len(words) < 2 or words[1] in {"-c", "-"}:
                return False
            if words[1] == "-m":
                if len(words) < 3 or words[2] != "pytest" or not _safe_pytest_args(root, words[3:]):
                    return False
            elif words[1].startswith("-") or not words[1].endswith(".py"):
                return False
            else:
                try:
                    (root / words[1]).resolve(strict=False).relative_to(root)
                except (OSError, ValueError):
                    return False
                if not _safe_python_cli_args(root, words[2:]):
                    return False
        elif executable in {"pytest", "py.test"}:
            if not _safe_pytest_args(root, words[1:]):
                return False
        elif executable == "echo":
            if words != ["echo", "exit:STATUS"]:
                return False
        elif executable == "tail":
            if len(words) != 2 or not re.fullmatch(r"-[0-9]+", words[1]):
                return False
        else:
            return False
        if executable == "tail" and index != len(segments) - 1:
            return False
    return True


def _safe_cat_operands(words: list[str], workspace_root: str | None) -> bool:
    """Prove that every ``cat`` operand stays inside the bound workspace.

    The proof resolves the operand, so a relative symlink to ``/etc`` is
    rejected.  Cortex's private store is excluded even when it is physically
    below the project root; coordinator reads must use the runtime's store
    APIs instead.  Missing files are allowed only when their resolved parent
    remains inside the workspace (the command may legitimately report a
    missing project file without gaining outside access).
    """
    if not workspace_root:
        return False
    try:
        root = Path(workspace_root).resolve(strict=True)
    except OSError:
        return False
    for operand in words:
        if not _SAFE_RELATIVE_READ_PATH.fullmatch(operand) or operand.startswith("-"):
            return False
        if any(part == ".cortex" for part in Path(operand).parts):
            return False
        try:
            resolved = (root / operand).resolve(strict=False)
            resolved.relative_to(root)
        except (OSError, ValueError):
            return False
        # A symlink in the worker workspace can otherwise make a private
        # store file look like an ordinary project-relative read.  Check the
        # resolved path as well as the spelling above; the native engine may
        # have declared only the symlink operand.
        if ".cortex" in resolved.relative_to(root).parts:
            return False
    return True


def _private_store_candidate(candidate: str, root: Path) -> bool:
    """Return whether *candidate* names or resolves into ``root/.cortex``.

    This is deliberately a path check for a declared ACP tool request.  It
    does not claim to inspect arbitrary code run by a native terminal tool.
    Both the lexical and resolved forms matter: a worker can name a direct
    ``.cortex`` path or a harmless-looking symlink whose target is there.
    """
    if not isinstance(candidate, str) or not candidate:
        return False
    try:
        spelled = Path(candidate) if Path(candidate).is_absolute() else root / candidate
        resolved = spelled.resolve(strict=False)
    except (OSError, RuntimeError, ValueError):
        return False
    return ".cortex" in spelled.parts or ".cortex" in resolved.parts


def worker_private_store_access(params, workspace_root: str) -> str | None:
    """Return the first declared worker path that reaches private storage.

    Native ``Read``/search requests normally arrive as ``kind=read`` with a
    ``locations`` entry or a path in ``rawInput``.  Claude may instead label
    a shell request ``other``; for that form every shell token is checked,
    including symlink targets.  A request that reaches this helper is
    refused locally, before a host approval can turn it into access.  Code
    that hides a read inside an arbitrary command remains outside ACP's
    observable permission boundary.
    """
    tool_call = _tool_call(params)
    if is_cortex_own_tool(params):
        return None
    try:
        root = Path(workspace_root).resolve(strict=True)
    except (OSError, RuntimeError, ValueError):
        return None
    candidates: list[str] = []
    for location in tool_call.get("locations") or ():
        if isinstance(location, Mapping) and isinstance(location.get("path"), str):
            candidates.append(location["path"])
    raw_input = tool_call.get("rawInput")
    if isinstance(raw_input, Mapping):
        candidates.extend(
            raw_input[key] for key in _TOOL_PATH_KEYS
            if isinstance(raw_input.get(key), str)
        )
    if tool_call.get("kind") in {"other", "execute"}:
        command = _command_text_only(tool_call)
        try:
            candidates.extend(shlex.split(command, posix=True))
        except ValueError:
            # An unparseable command is handled by the ordinary fail-closed
            # ``requires_confirmation`` path; it cannot be proved private.
            pass
    for candidate in candidates:
        if _private_store_candidate(candidate, root):
            return candidate
    return None


def _is_observational_shell_command(text: str, workspace_root: str | None = None) -> bool:
    """Return true only for a narrow, obviously non-mutating shell chain.

    This exists for ACP adapters that label Bash as ``other``. It never
    treats arbitrary text as safe: only ``cat`` of project-relative paths,
    the literal marker ``echo ---``, and ``git status`` with a small set of
    display-only flags are recognized. A missing command, absolute/parent
    path, shell construct, option, redirect, or unknown executable remains
    unclassifiable and therefore refused.
    """
    if not isinstance(text, str) or not text.strip():
        return False
    if _is_read_only_python_heredoc(text):
        return True
    if _READ_ONLY_SHELL_UNSAFE.search(text):
        return False
    commands = [part.strip() for part in text.split("&&")]
    if any(not part for part in commands):
        return False
    for index, command in enumerate(commands):
        words = command.split()
        if not words or Path(words[0]).name != words[0]:
            return False
        executable = words[0]
        if executable == "cat":
            if len(words) < 2 or not _safe_cat_operands(words[1:], workspace_root):
                return False
        elif executable == "echo":
            if words != ["echo", "---"]:
                return False
        elif executable == "git":
            if words not in (["git", "status"], ["git", "status", "--short"],
                             ["git", "status", "--porcelain"]):
                return False
        else:
            return False
        # Keep the accepted sequence intentionally narrow: cat/read marker,
        # optional marker, then status. This also prevents adding a second
        # otherwise-safe command whose interaction has not been reviewed.
        if index == 0 and executable != "cat":
            return False
        if index == len(commands) - 1 and executable != "git":
            return False
    return True


def _is_coordinator_safe_check_chain(text: str, workspace_root: str | None = None) -> bool:
    """Recognize bounded coordinator status/test checks sent as Bash.

    Claude ACP labels these requests ``kind=other``. The accepted grammar is
    intentionally limited to ``&&`` chains of git status, unittest, and the
    repository focus-render checker. It permits that checker’s disposable
    ``/tmp/coord-*`` output directory, while rejecting private-store paths,
    traversal, redirects and destructive/API spellings.
    """
    if not isinstance(text, str) or not text.strip() or not isinstance(workspace_root, str):
        return False
    if _contains_observable_destructive_api(text) or _DELETION_COMMAND_PATTERN.search(text):
        return False
    if any(op in text for op in (";", "||", "|", ">", "<", "`", "$")):
        return False
    try:
        root = Path(workspace_root).resolve(strict=True)
    except (OSError, RuntimeError, ValueError):
        return False
    for segment in text.split("&&"):
        try:
            words = shlex.split(segment.strip(), posix=True)
        except ValueError:
            return False
        if not words:
            return False
        executable = Path(words[0]).name
        if executable == "git":
            if len(words) < 2 or words[1] != "status":
                return False
            allowed = {"--porcelain", "--short"}
            for word in words[2:]:
                if word in allowed:
                    continue
                normalized = word.rstrip("/")
                if not normalized or not _SAFE_RELATIVE_READ_PATH.fullmatch(normalized) or normalized.startswith(".cortex"):
                    return False
                try:
                    (root / normalized).resolve(strict=False).relative_to(root)
                except (OSError, RuntimeError, ValueError):
                    return False
            continue
        if executable != "python3":
            return False
        args = words[1:]
        if len(args) >= 2 and args[:2] == ["-m", "unittest"]:
            args = args[2:]
            if not args or any(not re.fullmatch(r"[A-Za-z0-9_.-]+", arg) or arg.startswith(".cortex") for arg in args):
                return False
            continue
        if not args or args[0].startswith("-") or not args[0].endswith(".py"):
            return False
        try:
            (root / args[0]).resolve(strict=False).relative_to(root)
        except (OSError, RuntimeError, ValueError):
            return False
        index = 1
        while index < len(args):
            if args[index] != "--output-dir" or index + 1 >= len(args):
                return False
            target = args[index + 1]
            if not re.fullmatch(r"/tmp/coord-[A-Za-z0-9_.-]+", target):
                return False
            index += 2
    return True


def _is_coordinator_non_destructive_bash(params, workspace_root: str | None) -> bool:
    """Apply the worker lexical deletion policy to coordinator Bash.

    Coordinator Bash is allowed to contain ordinary test/status plumbing
    (including ``;``, ``2>&1`` and ``$?``). Private-store operands and
    absolute paths outside the disposable coordinator output directory stay
    guarded. This remains lexical: arbitrary interpreter code can hide an
    effect that ACP did not expose.
    """
    if not isinstance(workspace_root, str):
        return False
    if _declared_path_outside_workspace(params, workspace_root) is not None:
        return False
    tool_call = _tool_call(params)
    text = _command_text_only(tool_call)
    if not text.strip() or _contains_observable_destructive_api(text):
        return False
    # Claude sometimes wraps ordinary checks in a native shell launcher.
    # Unwrap only the literal ``-c``/``-lc`` form; malformed or composed
    # wrappers remain opaque and therefore continue to require review.
    try:
        wrapper_words = shlex.split(text, posix=True)
    except ValueError:
        return False
    if len(wrapper_words) >= 3 and Path(wrapper_words[0]).name in {"bash", "sh"} and wrapper_words[1] in {"-c", "-lc"}:
        text = wrapper_words[2]
        if len(wrapper_words) != 3 or not text.strip():
            return False
        if _contains_observable_destructive_api(text):
            return False
    if _DELETION_COMMAND_WITHOUT_REDIRECT.search(text) or _DIRECT_DESTRUCTIVE_WORD.search(text):
        return False
    # fd duplication is not a truncating redirect; normalize it before the
    # redirect check so ``2>&1`` in test commands does not prompt.
    normalized = re.sub(r"\d*>&\d+", "", text)
    if _DELETION_COMMAND_PATTERN.search(normalized):
        return False
    if worker_private_store_access(params, workspace_root) is not None:
        return False
    try:
        root_text = str(Path(workspace_root).resolve(strict=True))
    except (OSError, RuntimeError, ValueError):
        return False
    try:
        words = shlex.split(normalized, posix=True)
    except ValueError:
        return False
    # No executable/syntax allowlist: ordinary ACP commands may contain
    # loops, pipes, and tools unknown to Cortex. Only deletion, private-store,
    # traversal, and explicit outside-path boundaries are enforced here.
    for word in words:
        if word.startswith("/") and word not in {root_text} and not re.fullmatch(r"/tmp/coord-[A-Za-z0-9_.-]+", word):
            return False
        if word == ".." or word.startswith("../") or "/../" in word:
            return False
        if not word.startswith(("-", "/", "$", "'", '"')) and "/" not in word and "=" not in word:
            candidate = Path(workspace_root) / word
            try:
                if candidate.is_symlink() or candidate.exists():
                    candidate.resolve(strict=True).relative_to(Path(workspace_root).resolve(strict=True))
            except (OSError, RuntimeError, ValueError):
                return False
    return True


def _tool_call(params) -> dict:
    tool_call = (params or {}).get("toolCall") if isinstance(params, dict) else None
    return tool_call if isinstance(tool_call, dict) else {}


def _command_text(tool_call: dict) -> str:
    raw_input = tool_call.get("rawInput")
    parts = []
    if isinstance(raw_input, dict):
        for key in ("command", "cmd", "script"):
            value = raw_input.get(key)
            if isinstance(value, str):
                parts.append(value)
            elif isinstance(value, list):
                parts.append(" ".join(str(item) for item in value))
    title = tool_call.get("title")
    if isinstance(title, str):
        parts.append(title)
    return " ".join(parts)


# This runtime's own MCP tools, however an engine spells the server
# prefix ("mcp__cortex__delegate", "mcp.cortex.create_task", ...). They
# are Cortex's own control surface, not an action on the project.
_CORTEX_MCP_TOOL = re.compile(r"(?:^|[^A-Za-z0-9])mcp[._]{1,2}cortex[._]{1,2}", re.I)

# Engine-native skill trees are private implementation state of the child
# engine.  A coordinator must never read or execute them: doing so both leaks
# host-private instructions and is the source of the recurring Claude request
# to run ``~/.codex/skills/playwright/scripts/playwright_cli.sh``.  This is a
# lexical check over the exact ACP payload only; it does not pretend to parse
# arbitrary shell code or inspect the host filesystem.
_ENGINE_PRIVATE_PATH = re.compile(
    r"(?:^|[\s'\"=:])(?:~|\$HOME|/home/[^\s'\"=:]+|/root)?/?\.(?:codex|claude)(?:[/\s'\"=:]|$)"
    r"|(?:^|[\s'\"=:])\.(?:codex|claude)(?:[/\s'\"=:]|$)",
    re.IGNORECASE,
)


def coordinator_engine_private_access(params) -> str | None:
    """Return an engine-private path exposed by a coordinator request.

    Only ACP-declared locations and command text are inspected.  The check is
    deliberately limited to the native ``.codex``/``.claude`` trees instead
    of attempting to classify arbitrary shell behavior.
    """
    tool_call = _tool_call(params)
    values: list[str] = []
    for location in tool_call.get("locations") or ():
        if isinstance(location, Mapping) and isinstance(location.get("path"), str):
            values.append(location["path"])
    raw_input = tool_call.get("rawInput")
    if isinstance(raw_input, Mapping):
        values.extend(
            raw_input[key] for key in _TOOL_PATH_KEYS
            if isinstance(raw_input.get(key), str)
        )
    if tool_call.get("kind") in {"other", "execute"}:
        values.append(_command_text_only(tool_call))
    for value in values:
        if _ENGINE_PRIVATE_PATH.search(value):
            return value
    return None


def _coordinator_runs_project_check(params) -> bool:
    """Recognize common, explicitly named project test/check invocations.

    This is intentionally a small token check, not a shell allowlist.  It
    covers the native test runners and repository check scripts seen in ACP
    payloads while leaving ordinary public reads/status commands alone.
    """
    tool_call = _tool_call(params)
    if tool_call.get("kind") not in {"other", "execute"}:
        return False
    text = _command_text_only(tool_call)
    if not text.strip():
        return False
    try:
        words = shlex.split(text, posix=True)
    except ValueError:
        return False
    # Claude commonly wraps the actual command in ``/bin/sh -c``.  Inspect
    # only this literal wrapper and its one opaque command argument; do not
    # attempt to interpret arbitrary shell syntax.
    if (
        len(words) == 3
        and Path(words[0]).name in {"sh", "bash"}
        and words[1] in {"-c", "-lc"}
    ):
        text = words[2]
        try:
            words = shlex.split(text, posix=True)
        except ValueError:
            return False
    # A coordinator may inspect public source, but a small inline assertion
    # script is still a project check even when it avoids pytest/unittest.
    # Restrict this to an explicit check marker plus a project/UI target so a
    # harmless ``python3 -c 'print(...)'`` or source-reading command remains
    # ordinary coordinator inspection.
    check_marker = re.search(
        r"\b(?:assert(?:ion|equal|true|false)?|contrast(?:_ratio)?|relative[_ -]?luminance|wcag|"
        r"getcomputedstyle|queryselector|beautifulsoup|dom|focus-visible|responsive|aria[-_]?[a-z-]+)\b",
        text,
        re.IGNORECASE,
    )
    project_target = re.search(
        r"(?:^|[\s/'\"=])(?:[^\s/'\"=]+/)?(?:index\.html|[^\s/'\"=]*\.css|tests?/|test_[^\s/'\"=]*\.py)",
        text,
        re.IGNORECASE,
    )
    if check_marker and project_target:
        return True
    # Grep/rg is a normal source read unless its pattern clearly names a
    # rendered/UI assertion.  These markers cover the exact focus/semantic/
    # link checks used by the live landing-page qualification without turning
    # every repository search into a coordinator refusal.
    if re.search(r"\b(?:grep|rg)\b", text, re.IGNORECASE) and re.search(
        r"focus-visible|contrast|aria|role=|href=|responsive|@media|button|semantic|tabindex",
        text,
        re.IGNORECASE,
    ) and project_target:
        return True
    if re.search(r"\b(?:grep|rg)\b", text, re.IGNORECASE) and re.search(
        r"(?:^|\s)(?:-[A-Za-z]*q[A-Za-z]*|--quiet|--count)(?:\s|$)", text,
        re.IGNORECASE,
    ) and project_target:
        return True
    for index, word in enumerate(words):
        executable = Path(word).name.lower()
        if executable in {"pytest", "py.test", "unittest"}:
            return True
        if executable in {"python", "python3", "python3.11", "python3.12"}:
            if index + 2 < len(words) and words[index + 1] == "-m" and words[index + 2].lower() in {"pytest", "unittest"}:
                return True
            for argument in words[index + 1:]:
                normalized = argument.replace("\\", "/").lower()
                if "/tests/" in f"/{normalized.strip('/')}/" or normalized.startswith("tests/"):
                    return True
                if Path(normalized).name.startswith(("test_", "check_")) and normalized.endswith(".py"):
                    return True
        if executable in {"npm", "pnpm", "yarn", "bun"} and index + 1 < len(words):
            if words[index + 1].lower() in {"test", "check", "lint"}:
                return True
    return False


def is_cortex_own_tool(params) -> bool:
    """True when this permission request is for one of Cortex's own MCP
    tools -- ``delegate``, ``set_governance``, ``create_task`` and the
    rest.

    They carry no ACP ``kind`` of their own (an MCP call is ``"other"``),
    and an unrecognized kind is refused for a coordinator by design. Live,
    on claude-agent-acp -- which asks permission for every tool, including
    MCP ones -- that refused the coordinator's own ``delegate`` and
    ``set_governance``: it could not delegate, could not set governance,
    and ended its turn asking the user to grant tool access. Codex never
    surfaced it because it does not route MCP calls through this prompt.

    Recognized by tool name only (``toolCall.title``/``rawInput`` naming
    the server-prefixed tool), never by kind -- the point is precisely
    that the kind says nothing.
    """
    tool_call = _tool_call(params)
    for value in (tool_call.get("title"), tool_call.get("name"), (params or {}).get("toolName")):
        if isinstance(value, str) and _CORTEX_MCP_TOOL.search(value):
            return True
    raw_input = tool_call.get("rawInput")
    if isinstance(raw_input, dict):
        for key in ("name", "tool", "toolName"):
            value = raw_input.get(key)
            if isinstance(value, str) and _CORTEX_MCP_TOOL.search(value):
                return True
    return False


def requires_confirmation(params) -> bool:
    """True unless this ``session/request_permission`` call is confidently
    classified as *not* a file deletion -- the default is to ask.

    - ``toolCall.kind`` of ``"delete"``/``"move"`` (ACP's own vocabulary)
      is always a deletion.
    - ``toolCall.kind`` of ``"execute"`` is a deletion only when its
      ``rawInput`` command text (or, failing that, its ``title``) matches
      a known destructive shell idiom (``rm``, ``rmdir``, ``unlink``,
      ``shred``, ``git clean``, ``find ... -delete``, ``mv``, or a
      truncating ``>`` redirect) -- see the module-level pattern and its
      own caveat.
    - Every other recognized ``toolCall.kind`` (``read``, ``edit``,
      ``search``, ``fetch``, ``think``, ``switch_mode``) is never a
      deletion.
    - ACP's catch-all ``"other"`` is treated like a native Bash/terminal
      request: the declared command is scanned for the known destructive
      shell idioms.  This keeps ordinary worker Bash (including opaque
      read-only heredocs) from being blocked by a syntax allowlist.  The
      scan remains lexical; code hidden inside an arbitrary interpreter can
      still have effects that ACP did not expose, and is outside this
      bridge's prevention guarantee.
    - A missing, null, or other unrecognized ``kind`` is never guessed at --
      it is always treated as a deletion (i.e. this returns ``True``), so
      an unclassifiable request always asks rather than silently proceeding.
    """
    if is_cortex_own_tool(params):
        # Cortex's own MCP surface deletes nothing: a worker publishing a
        # result or reading a skill must not be held for confirmation
        # merely because an MCP call carries no ACP kind.
        return False
    tool_call = _tool_call(params)
    kind = tool_call.get("kind")
    if kind in _DELETION_TOOL_KINDS:
        return True
    if kind == "execute":
        text = re.sub(r"\d*>&\d+", "", _command_text(tool_call))
        if _contains_observable_destructive_api(text):
            return True
        if _DELETION_COMMAND_WITHOUT_REDIRECT.search(text) or _DIRECT_DESTRUCTIVE_WORD.search(text):
            return True
        if _redirects_only_where_nothing_can_be_lost(text):
            return False
        return bool(_DELETION_COMMAND_PATTERN.search(text))
    if kind == "other":
        text = re.sub(r"\d*>&\d+", "", _command_text(tool_call))
        if not text.strip():
            return True
        if _contains_observable_destructive_api(text):
            return True
        if _DELETION_COMMAND_WITHOUT_REDIRECT.search(text) or _DIRECT_DESTRUCTIVE_WORD.search(text):
            return True
        if _redirects_only_where_nothing_can_be_lost(text):
            return False
        return bool(_DELETION_COMMAND_PATTERN.search(text))
    if kind in _NON_DELETION_TOOL_KINDS:
        return False
    return True


# T035 FINDING-12 closure ("the coordinator must not be able to work on the
# project" -- see this module's own module docstring and
# configuration.policy.COORDINATOR_MODE_FLOOR's docstring for the live
# evidence that an engine-native non-editing mode does not enforce this):
# reuses requires_confirmation's own toolCall.kind buckets and its
# command-text inspection for "execute" rather than inventing a second
# classification scheme. Every request requires_confirmation already
# treats as ask-worthy for a worker (a deletion, an unrecognized/missing
# kind, or a destructive shell idiom under "execute") is marked as unsafe
# here for a coordinator and sent to a connected host user for one-shot
# review. Missing/denied review still refuses the intent. The one addition:
# toolCall.kind == "edit" is never a deletion (requires_confirmation
# returns False for it -- a worker may edit freely) but is always a
# mutation, so it is refused here too.
def is_coordinator_mutating(params, *, workspace_root: str | None = None) -> bool:
    """True when this request requires explicit user review for a
    coordinator binding rather than automatic approval.

    Explicit about what counts as mutating: ``toolCall.kind`` of
    ``"delete"``/``"move"`` always is; a declared ``"edit"`` fully inside
    the bound project is ordinary work and is auto-approved, while an edit
    outside it or into ``.cortex`` remains mutating; a missing/unrecognized
    kind (including ACP's own ``"other"``) always is (an unclassifiable
    request requires review, never guessed at -- see module docstring);
    Engine-private skill paths remain a hard coordinator boundary even when
    they are otherwise non-destructive; the bridge handles those locally and
    does not relay them to a host. Safe, project-bound test/check commands are
    ordinary coordinator inspection and are auto-approved under the same
    lexical outside/private/deletion guards as other Bash. ``"execute"`` is only
    when its command text matches
    :data:`_DELETION_COMMAND_PATTERN`. Every other recognized kind
    (``read``/``search``/``fetch``/``think``/``switch_mode``) -- and an
    ``"execute"`` call whose command text matches none of that pattern --
    is *not* refused, so a coordinator can still inspect the repository
    (list/read/search/fetch/run an ordinary non-destructive shell command)
    to decide what to delegate.

    HONEST BOUNDARY (the same one :func:`requires_confirmation` already
    discloses, reused as-is here): the ``"execute"`` command-text scan only
    ever recognizes a known *destructive* idiom; a non-destructive *write*
    under ``"execute"`` (an append, ``touch``, ``mkdir``, a package
    install, ...) is not caught by this reused pattern and is not refused.
    This is the same documented incompleteness as the deletion
    classifier's own, not a new gap this function introduces.
    """
    if is_cortex_own_tool(params):
        # Never a project mutation: this is the coordinator using the
        # runtime it is a coordinator *of*.
        return False
    # Engine-private skill trees and project checks are hard coordinator
    # boundaries.  They are refused locally by PermissionBridge, rather than
    # being presented as a host permission question.
    if coordinator_engine_private_access(params) is not None:
        return True
    if _coordinator_private_or_outside_access(params, workspace_root) is not None and not requires_confirmation(params):
        return True
    tool_call = _tool_call(params)
    if tool_call.get("kind") == "edit":
        # Coordinator source edits are a role violation. The exact registered
        # pipeline draft is handled by PermissionBridge before this predicate.
        return True
    if tool_call.get("kind") in {"other", "execute"}:
        # Coordinator Bash follows the same ordinary-action/deletion policy
        # as workers, with private-store and outside-path guards. This keeps
        # harmless compound checks from becoming syntax special cases.
        if _is_coordinator_non_destructive_bash(params, workspace_root):
            return False
        return not _is_observational_shell_command(
            _command_text_only(tool_call), workspace_root,
        )
    return requires_confirmation(params)


# T035 FINDING-12 closure: the observability-log reason recorded alongside
# every coordinator mutation refusal (see PermissionBridge._handle_one).
# ACP's RequestPermissionResponse has no text field at all (verified against
# the vendored tests/fixtures/acp/schema-1.4.0.json -- RequestPermissionOutcome
# is only ever {"outcome":"cancelled"} or {"outcome":"selected","optionId":
# ...}; its "_meta" property is explicitly documented as something "clients
# MUST NOT make assumptions about"), so this text cannot be attached to the
# wire reply itself -- see module docstring's own HONEST BOUNDARY. It is
# delivered to the coordinator model instead through the same continuation-
# text channel every other runtime-authored explanation already uses (see
# ``delegation.live_adapters.EngineSessionManagerAdapter``'s
# ``coordinator_refusal_notifier`` wiring and ``acp.orchestrating_runtime
# ._render_continuation_text``'s "progress" handling): the coordinator reads
# it, as ordinary continuation text, the next time its own engine session is
# prompted. Local role refusals make that next prompt an immediate
# continuation in the same host turn; ACP still cannot inject the reason
# synchronously into the in-flight permission response.
COORDINATOR_MUTATION_REFUSAL_REASON = (
    "This mutating action is outside the coordinator's automatic policy and requires "
    "explicit one-shot user review. Delegate it to a worker unless the user explicitly "
    "approves the exact action."
)

COORDINATOR_EDIT_REFUSAL_REASON = (
    "Cortex coordinator role restriction: you cannot edit project source files. "
    "This is a local Cortex policy decision, not a user permission request. "
    "Delegate implementation to a worker through Cortex and continue the task; "
    "do not ask the user to approve the coordinator's edit."
)

COORDINATOR_PRIVATE_OR_CHECK_REFUSAL_REASON = (
    "Coordinator requests cannot access engine-private skill directories or paths outside the opened project. "
    "Use the Cortex MCP workflow and delegate implementation and verification to a worker."
)


def _auto_allow_option_id(params):
    """Return the offered allow option for an automatically safe request.

    Claude ACP can expose only ``allow_always`` for a harmless Bash request,
    while Codex commonly offers ``allow_once``.  Cortex still makes the
    decision locally for this request; it does not persist a Cortex-side
    standing rule.  Prefer the one-shot option when available, but accept the
    engine's only valid allow option when that is the shape it offered.  A
    deletion never reaches this helper: its caller routes it through the
    host review path and ``_validate_host_response`` still rejects persistent
    choices there.
    """
    options = (params or {}).get("options") if isinstance(params, dict) else None
    if not isinstance(options, list):
        return None
    for kind in ("allow_once", "allow_always"):
        for option in options:
            if isinstance(option, dict) and option.get("kind") == kind:
                return option.get("optionId")
    return None


def _auto_reject_option_id(params):
    """The offered ``optionId`` whose own ``kind`` is ``"reject_once"``
    (preferred) or, failing that, ``"reject_always"`` -- or ``None`` if the
    engine offered neither, in which case the caller falls back to
    ``CANCELLED_OUTCOME``. Never invents an id the engine did not itself
    offer (same invariant as :func:`_auto_allow_option_id` /
    :meth:`PermissionBridge._validate_host_response`). ``"reject_once"`` is
    preferred over ``"reject_always"`` for the same reason
    ``_auto_allow_option_id`` prefers ``"allow_once"`` but may use an
    engine-offered ``"allow_always"`` when it is the only allow option. The
    bridge still makes a fresh local decision and does not persist a standing
    Cortex rule."""
    options = (params or {}).get("options") if isinstance(params, dict) else None
    if not isinstance(options, list):
        return None
    by_kind = {
        option.get("kind"): option.get("optionId")
        for option in options if isinstance(option, dict)
    }
    return by_kind.get("reject_once") or by_kind.get("reject_always")


class DuplicateEnginePermissionRequestId(Exception):
    """The engine reused a still-outstanding (or never-cleaned-up) wire
    request id for ``session/request_permission``; refused as a protocol
    error, never treated as a fresh request."""


class PermissionBridge:
    def __init__(
        self, *, correlation, bindings, logs=None, host_permission_requester=None, call_timeout=30.0,
        coordinator_refusal_notifier=None, permission_receipt_sink=None,
        coordinator_draft_write_authorizer=None, worker_draft_write_authorizer=None,
    ):
        """``host_permission_requester`` -- ``(engine_session_id, params,
        timeout) -> dict`` or ``None`` -- is the caller-injected seam to the
        actual incoming ACP session/host that owns this engine session (see
        ``acp.host_requests.HostRequester``); when ``None`` every permission
        request is answered ``cancelled`` without ever reaching a host.

        ``coordinator_refusal_notifier`` -- ``(engine_session_id, reason) ->
        None`` or ``None`` (default) -- is called, synchronously and never
        allowed to raise into the caller (see :meth:`_notify_refusal`),
        every time this bridge refuses a coordinator binding's mutating
        request (see :func:`is_coordinator_mutating`). This is the only
        way ``COORDINATOR_MUTATION_REFUSAL_REASON`` ever reaches anything
        beyond this process's own observability log -- see that constant's
        own module-level docstring for exactly how (and when) it reaches
        the coordinator model."""
        self._correlation = correlation
        self._bindings = bindings
        self._logs = logs
        self._host_permission_requester = host_permission_requester
        self._call_timeout = call_timeout
        self._coordinator_refusal_notifier = coordinator_refusal_notifier
        self._permission_receipt_sink = permission_receipt_sink
        self._coordinator_draft_write_authorizer = coordinator_draft_write_authorizer
        self._worker_draft_write_authorizer = worker_draft_write_authorizer
        self._lock = threading.Lock()
        self._in_flight: set[tuple[str, object]] = set()
        # T035 EXT-01 action-supersession gap closure: which portable
        # action refs (see ``_action_ref``) currently have a host round
        # trip outstanding, indexed by wire session id -- a plain refcount
        # rather than a set because two concurrent requests can legitimately
        # name the *same* toolCallId (an engine retry while the first
        # round trip is still outstanding), and the second's own eventual
        # completion must never make the first look "no longer pending" out
        # from under it. The only reader is
        # :meth:`pending_actions_for_wire_session`, called by
        # ``session_manager.EngineSessionManager`` at the one place a whole
        # turn (and everything it requested permission for) is abandoned or
        # explicitly cancelled -- see that method's own docstring for why
        # this bridge, not that caller, is the one with this data at hand.
        self._pending_actions: dict[str, dict[str, int]] = {}

    def pending_actions_for_wire_session(self, wire_session_id: str) -> list[str]:
        """Snapshot of every action ref with a host round trip currently
        outstanding for ``wire_session_id`` -- see
        ``session_manager.EngineSessionManager._abandon_turn``/``cancel``,
        the only production callers (T035 EXT-01 action-supersession gap
        closure)."""
        with self._lock:
            return list(self._pending_actions.get(wire_session_id, {}))

    def _track_action_start(self, wire_session_id, action_ref) -> None:
        if action_ref is None:
            return
        with self._lock:
            by_action = self._pending_actions.setdefault(wire_session_id, {})
            by_action[action_ref] = by_action.get(action_ref, 0) + 1

    def _track_action_end(self, wire_session_id, action_ref) -> None:
        if action_ref is None:
            return
        with self._lock:
            by_action = self._pending_actions.get(wire_session_id)
            if not by_action or action_ref not in by_action:
                return
            by_action[action_ref] -= 1
            if by_action[action_ref] <= 0:
                del by_action[action_ref]
            if not by_action:
                self._pending_actions.pop(wire_session_id, None)

    def _observe(self, event, **fields) -> None:
        if self._logs is not None:
            self._logs.observe(event, **fields)

    def _notify_refusal(self, engine_session_id: str, reason: str = COORDINATOR_MUTATION_REFUSAL_REASON) -> None:
        if self._coordinator_refusal_notifier is None:
            return
        try:
                self._coordinator_refusal_notifier(engine_session_id, reason)
        except Exception:  # noqa: BLE001 - a notifier failure must never affect the refusal itself
            self._observe("coordinator_refusal_notifier_failed", engine_session_id=engine_session_id)

    def _ask_host_after_refusal(self, *, engine_session_id, binding, action_ref, params) -> dict | None:
        """Give the connected user the final say over a policy refusal.

        This is deliberately a narrow escape hatch: it is used only after
        the bridge has classified an otherwise actionable request as unsafe
        or ambiguous.  A worker path that names a file outside its bound
        workspace never calls this method; that is a hard containment deny.
        The user response is still checked against the current binding and
        the engine's own one-shot options, so a stale, persistent, malformed,
        or missing decision cannot authorize anything.
        """
        if self._host_permission_requester is None:
            self._observe("permission_no_host_requester", engine_session_id=engine_session_id)
            return None
        try:
            host_response = self._host_permission_requester(
                engine_session_id, _named_for_the_user(params), self._call_timeout,
            )
        except Exception:  # noqa: BLE001 - host timeout/EOF is a denial
            self._observe("permission_host_request_failed", engine_session_id=engine_session_id)
            return None
        if not binding.is_current(self._bindings.snapshot(binding, action_key=action_ref)):
            self._observe("late_permission_response", engine_session_id=engine_session_id)
            return None
        return self._validate_host_response(host_response, params, require_one_shot=True)

    def handle(self, *, engine_session_id, wire_session_id, wire_request_id, params) -> dict:
        """Returns the JSON-RPC *result* payload to send back to the
        engine. Raises :class:`DuplicateEnginePermissionRequestId` for a
        wire request id already in flight on this engine session -- the
        caller (see ``session_manager``) turns that into a JSON-RPC error
        reply, never a result."""
        dedup_key = (engine_session_id, wire_request_id)
        with self._lock:
            if dedup_key in self._in_flight:
                self._observe("permission_duplicate_request_id", engine_session_id=engine_session_id)
                raise DuplicateEnginePermissionRequestId(str(wire_request_id))
            self._in_flight.add(dedup_key)
        action_ref = self._action_ref(wire_session_id, params)
        self._track_action_start(wire_session_id, action_ref)
        try:
            return self._handle_one(engine_session_id, wire_session_id, params, action_ref)
        finally:
            with self._lock:
                self._in_flight.discard(dedup_key)
            self._track_action_end(wire_session_id, action_ref)

    def _action_ref(self, wire_session_id, params):
        tool_call = (params or {}).get("toolCall") if isinstance(params, dict) else None
        wire_tool_call_id = (tool_call or {}).get("toolCallId") if isinstance(tool_call, dict) else None
        if not wire_tool_call_id or not wire_session_id:
            return None
        return self._correlation.portable_tool_call_ref(wire_session_id, wire_tool_call_id)

    def _handle_one(self, engine_session_id, wire_session_id, params, action_ref) -> dict:
        binding = self._bindings.get(engine_session_id)
        if binding is None:
            self._observe("permission_denied_no_binding", engine_session_id=engine_session_id)
            return dict(CANCELLED_OUTCOME)
        snapshot = self._bindings.snapshot(binding, action_key=action_ref)
        if not binding.is_current(snapshot):
            self._observe("permission_denied_stale_binding", engine_session_id=engine_session_id)
            return dict(CANCELLED_OUTCOME)
        if binding.role == "worker":
            if _tool_call(params).get("kind") in {"other", "execute"}:
                declared_outside = _declared_path_outside_workspace(params, binding.workspace_root)
                if declared_outside is not None:
                    self._observe(
                        "worker_declared_path_outside_workspace_review",
                        engine_session_id=engine_session_id, path=declared_outside,
                    )
                    # A worker must not silently decide an outside-project
                    # request.  It is a user decision boundary: surface the
                    # exact action through Paseo, including harmless reads
                    # such as ``find /``.  Destructive commands take this
                    # same route and remain subject to one-shot validation
                    # in ``_ask_host_after_refusal``.  Worker writes that
                    # are outside the bound workspace still hit the hard
                    # containment check below after this request-level
                    # review path.
                    host_result = self._ask_host_after_refusal(
                        engine_session_id=engine_session_id,
                        binding=binding,
                        action_ref=action_ref,
                        params=params,
                    )
                    if host_result is not None:
                        self._observe(
                            "worker_declared_path_outside_workspace_user_decision",
                            engine_session_id=engine_session_id,
                        )
                        return host_result
                    reject_option_id = _auto_reject_option_id(params)
                    if reject_option_id is not None:
                        return {"outcome": {"outcome": "selected", "optionId": reject_option_id}}
                    return dict(CANCELLED_OUTCOME)
            # Claude's native Write labels the worker's report draft as a
            # private-store edit.  Authorize only the exact active draft
            # returned by Cortex; all other .cortex access remains refused.
            if self._worker_draft_write_authorizer is not None:
                try:
                    draft_write = self._worker_draft_write_authorizer(engine_session_id, binding, params)
                except Exception:  # noqa: BLE001 - authorization fails closed
                    draft_write = False
                if draft_write:
                    option_id = _auto_allow_option_id(params)
                    if option_id is not None and binding.is_current(self._bindings.snapshot(binding, action_key=action_ref)):
                        self._observe("worker_draft_write_auto_approved", engine_session_id=engine_session_id)
                        return {"outcome": {"outcome": "selected", "optionId": option_id}}
            # A worker works on a copy, and its result reaches the project
            # only through the conflict-checked integration step. That was
            # true by convention alone -- the copy was its working
            # directory -- until a worker pointed its `Edit` at the
            # project's real path instead: its own copy stayed untouched,
            # there was no integration at all, and the coordinator accepted
            # a change nothing had checked. See
            # worker_write_outside_workspace, including what it does not
            # cover.
            private = worker_private_store_access(params, binding.workspace_root)
            if private is not None:
                self._observe(
                    "worker_private_store_access_refused",
                    engine_session_id=engine_session_id, path=private,
                )
                # Private reads/ordinary edits remain a hard deny. A
                # deletion is still sent to the host for the user's one-shot
                # confirmation, including when its target is in .cortex.
                tool_call = _tool_call(params)
                kind = tool_call.get("kind")
                command = _command_text_only(tool_call)
                deletion = kind in _DELETION_TOOL_KINDS or (
                    kind == "execute" and bool(_DELETION_COMMAND_PATTERN.search(command))
                )
                if not deletion:
                    reject_option_id = _auto_reject_option_id(params)
                    if reject_option_id is not None:
                        return {"outcome": {"outcome": "selected", "optionId": reject_option_id}}
                    return dict(CANCELLED_OUTCOME)
            outside = worker_write_outside_workspace(params, binding.workspace_root)
            if outside is not None:
                self._observe(
                    "worker_write_outside_workspace_refused",
                    engine_session_id=engine_session_id, path=outside,
                )
                reject_option_id = _auto_reject_option_id(params)
                if reject_option_id is not None:
                    return {"outcome": {"outcome": "selected", "optionId": reject_option_id}}
                return dict(CANCELLED_OUTCOME)
            worker_other_safe = (
                _tool_call(params).get("kind") == "other"
                and _is_worker_safe_other_command(
                    _command_text_only(_tool_call(params)), binding.workspace_root,
                )
            )
            worker_scoped_redirect = False
            if _tool_call(params).get("kind") in {"other", "execute"}:
                command = _command_text_only(_tool_call(params))
                worker_scoped_redirect = (
                    _redirects_only_inside_worker_workspace(command, binding.workspace_root)
                    and not _contains_observable_destructive_api(command)
                    and not _DELETION_COMMAND_WITHOUT_REDIRECT.search(command)
                )
            if worker_other_safe or worker_scoped_redirect or not requires_confirmation(params):
                option_id = _auto_allow_option_id(params)
                if option_id is not None:
                    self._observe("permission_auto_approved", engine_session_id=engine_session_id)
                    return {"outcome": {"outcome": "selected", "optionId": option_id}}
        if binding.role == "coordinator":
            # A pipeline draft is an owner-bound Cortex document, not project
            # source. Only the store-backed exact-path authorizer may make
            # this one edit exception; path prefixes or tool titles alone
            # never grant it. Failure of that lookup is a refusal.
            if _tool_call(params).get("kind") == "edit" and self._coordinator_draft_write_authorizer is not None:
                try:
                    draft_write = self._coordinator_draft_write_authorizer(engine_session_id, binding, params)
                except Exception:  # noqa: BLE001 - authorization fails closed
                    draft_write = False
                if draft_write:
                    option_id = _auto_allow_option_id(params)
                    if option_id is not None and binding.is_current(self._bindings.snapshot(binding, action_key=action_ref)):
                        self._observe("coordinator_pipeline_draft_auto_approved", engine_session_id=engine_session_id)
                        return {"outcome": {"outcome": "selected", "optionId": option_id}}
            # Native engine skill trees are a hard local coordinator boundary.
            # They must never become a Paseo host permission card: the
            # coordinator gets one model-visible continuation explaining that
            # the work belongs to a worker. Project-bound read-only tests and
            # checks are ordinary safe coordinator inspection and are handled
            # by the normal Bash classifier below.
            private_path = coordinator_engine_private_access(params)
            outside_path = _coordinator_private_or_outside_access(params, binding.workspace_root)
            if (private_path is not None or outside_path is not None) and not requires_confirmation(params):
                reason = COORDINATOR_PRIVATE_OR_CHECK_REFUSAL_REASON
                self._observe(
                    "coordinator_private_or_project_check_refused",
                    engine_session_id=engine_session_id,
                    reason=reason,
                    category="engine_private_skill" if private_path is not None else "outside_project",
                )
                self._notify_refusal(engine_session_id, reason)
                reject_option_id = _auto_reject_option_id(params)
                if reject_option_id is not None:
                    return {"outcome": {"outcome": "selected", "optionId": reject_option_id}}
                return dict(CANCELLED_OUTCOME)
            # A coordinator mutation is still marked as a policy refusal and
            # notified to the coordinator, but a connected user must be able
            # to review the exact action instead of receiving a silent
            # rejection.  This is a user-authorized one-shot escape hatch;
            # worker writes outside their bound workspace remain a hard deny
            # and never reach this path.
            if is_coordinator_mutating(params, workspace_root=binding.workspace_root):
                coordinator_edit = _tool_call(params).get("kind") == "edit"
                refusal_reason = (
                    COORDINATOR_EDIT_REFUSAL_REASON if coordinator_edit
                    else COORDINATOR_MUTATION_REFUSAL_REASON
                )
                self._observe(
                    "coordinator_mutation_refused", engine_session_id=engine_session_id,
                    reason=refusal_reason,
                )
                self._notify_refusal(engine_session_id, refusal_reason)
                if coordinator_edit:
                    reject_option_id = _auto_reject_option_id(params)
                    if reject_option_id is not None:
                        return {"outcome": {"outcome": "selected", "optionId": reject_option_id}}
                    return dict(CANCELLED_OUTCOME)
                host_result = self._ask_host_after_refusal(
                    engine_session_id=engine_session_id,
                    binding=binding,
                    action_ref=action_ref,
                    params=params,
                )
                if host_result is not None:
                    self._observe("coordinator_mutation_user_decision", engine_session_id=engine_session_id)
                    return host_result
                reject_option_id = _auto_reject_option_id(params)
                if reject_option_id is not None:
                    return {"outcome": {"outcome": "selected", "optionId": reject_option_id}}
                return dict(CANCELLED_OUTCOME)
            option_id = _auto_allow_option_id(params)
            if option_id is not None:
                self._observe("coordinator_read_auto_approved", engine_session_id=engine_session_id)
                return {"outcome": {"outcome": "selected", "optionId": option_id}}
            # No allow_once option offered for a request already classified
            # as non-mutating -- fail closed to cancelled rather than ever
            # relaying a coordinator's request to a host (a coordinator's
            # permission decisions are never a host's to make -- see
            # module docstring).
            self._observe("coordinator_read_no_allow_option", engine_session_id=engine_session_id)
            return dict(CANCELLED_OUTCOME)
        if self._host_permission_requester is None:
            self._observe("permission_no_host_requester", engine_session_id=engine_session_id)
            return dict(CANCELLED_OUTCOME)
        try:
            host_response = self._host_permission_requester(
                engine_session_id, _named_for_the_user(params), self._call_timeout,
            )
        except Exception:  # noqa: BLE001 - any host-side failure/timeout/EOF maps to cancelled
            self._observe("permission_host_request_failed", engine_session_id=engine_session_id)
            return dict(CANCELLED_OUTCOME)
        # Re-check binding currency immediately before relaying a selection:
        # never trust the snapshot taken before the (potentially slow) host
        # round-trip -- a session replacement/epoch bump/generation bump/
        # cancellation that happened *during* that round-trip must still
        # win. A late reply that loses this race is exactly the
        # "late_permission_response" case the caller/tests exercise.
        snapshot_after = self._bindings.snapshot(binding, action_key=action_ref)
        if not binding.is_current(snapshot_after):
            self._observe("late_permission_response", engine_session_id=engine_session_id)
            return dict(CANCELLED_OUTCOME)
        validated = self._validate_host_response(host_response, params)
        command = _command_text_only(_tool_call(params))
        if is_rm_command(command) and self._permission_receipt_sink is not None:
            # An approval is never returned to the engine if its exact
            # task/attempt/action receipt failed to persist. The sink sees
            # only a response received from the live host, after the binding
            # freshness check, never the model's own claim of permission.
            outcome = validated.get("outcome", {})
            option_id = outcome.get("optionId")
            options = params.get("options") if isinstance(params, dict) else None
            option = next((item for item in (options or ()) if isinstance(item, dict) and item.get("optionId") == option_id), None)
            host_outcome = option.get("kind") if option else "cancelled"
            try:
                self._permission_receipt_sink(
                    binding, action_ref, command, host_outcome,
                )
            except Exception:  # noqa: BLE001 - missing durable receipt cannot authorize an action
                self._observe("permission_receipt_failed", engine_session_id=engine_session_id)
                return dict(CANCELLED_OUTCOME)
        return validated

    def _validate_host_response(self, host_response, params, *, require_one_shot=False) -> dict:
        if not isinstance(host_response, dict):
            return dict(CANCELLED_OUTCOME)
        outcome = host_response.get("outcome")
        if not isinstance(outcome, dict) or outcome.get("outcome") != "selected":
            return dict(CANCELLED_OUTCOME)
        option_id = outcome.get("optionId")
        options = (params or {}).get("options") if isinstance(params, dict) else None
        offered_ids = {item.get("optionId") for item in options if isinstance(item, dict)} if isinstance(options, list) else set()
        if option_id not in offered_ids:
            self._observe("permission_unknown_option_id", option_id=option_id)
            return dict(CANCELLED_OUTCOME)
        if require_one_shot or requires_confirmation(params):
            chosen = next((item for item in options if isinstance(item, dict) and item.get("optionId") == option_id), None)
            if chosen is None or chosen.get("kind") != "allow_once" and chosen.get("kind") != "reject_once":
                self._observe("permission_persistent_choice_refused", option_id=option_id)
                return dict(CANCELLED_OUTCOME)
        return {"outcome": {"outcome": "selected", "optionId": option_id}}
