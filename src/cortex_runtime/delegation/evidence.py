"""T017 durable tool-call evidence over ``tool_result_observations`` (see
``delegation.evidence_store``) and the strict check-evidence rule.

Bridges an engine's own ``TOOL_CALL_OBSERVED`` updates (see
``engine.session_manager``/``portable.engine_contract.EngineEvent``) into a
durable per-attempt record, and assesses a worker's ``result_submit``
``checks`` against the runtime's own records (:func:`assess_check`):

- ``verified`` -- a passed check cites exactly one complete gateway
  ``command_run``/``outside_command`` receipt of this task and attempt, with
  exit code 0, a complete non-truncated retained output and a command that
  is not a trivial stand-in (:func:`trivial_command`). A failed check is
  verified by the same kind of receipt with a nonzero exit code.
- ``weak`` -- a passed or failed claim without such a receipt; ``gap``
  names the first unmet condition (:data:`EVIDENCE_GAPS`).
- ``none`` -- a ``not_run``/``unverified`` disclosure.

Engine-native tool calls are recorded here for display and audit, but the
runtime never observes their exit code or retains their complete output, so
they never verify a check. Nothing here marks a claim verified without the
runtime's own matching receipt from this exact attempt.
"""
from __future__ import annotations

from datetime import datetime, timezone
import hashlib


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def record_tool_result_observed(
    store, task_id: str, attempt_id: str, tool_call_ref: str | None, kind: str | None,
    title: str | None, status: str | None,
) -> None:
    """Durably record one observed tool-call status transition.

    A no-op when ``tool_call_ref`` or ``status`` is missing (an engine
    update the correlation table could not resolve, or with no status --
    nothing evidential to record).
    """
    if not tool_call_ref or not status:
        return
    bounded_title = (title or "")[:200] or None
    bounded_kind = (kind or "")[:64] or None

    def write(db):
        nonlocal bounded_kind, bounded_title
        if bounded_kind is None or bounded_title is None:
            earlier = db.execute(
                "SELECT kind, title FROM tool_result_observations "
                "WHERE task_id=? AND attempt_id=? AND tool_call_ref=? "
                "ORDER BY rowid ASC LIMIT 1",
                (task_id, attempt_id, tool_call_ref),
            ).fetchone()
            if earlier is not None:
                bounded_kind = bounded_kind or earlier["kind"]
                bounded_title = bounded_title or earlier["title"]
        db.execute(
            "INSERT INTO tool_result_observations (task_id, attempt_id, tool_call_ref, kind, title, status, created_at) "
            "VALUES (?,?,?,?,?,?,?)",
            (task_id, attempt_id, tool_call_ref, bounded_kind, bounded_title, status, _now()),
        )

    store.immediate_write(write)


# Every reason a passed (or failed) claim is not verified. The code is stored
# on a check the runtime kept unverified (``evidence_gap``) and derived again
# at read time for older results; the sentence is its readable blocker.
EVIDENCE_GAPS = {
    "tool_call_ref_missing": "no tool_call_ref was cited",
    "tool_call_ref_unknown": "the cited tool_call_ref is not a tool call of this task",
    "foreign_attempt": "the cited tool call belongs to a different attempt",
    "ref_already_cited": "the same tool_call_ref was already cited by an earlier check of this result",
    "not_a_command": "the cited tool call is not a command execution (a read, write, search, listing or report operation)",
    "engine_command_unverifiable": ("the cited call is an engine-native shell call whose exit code and complete "
                                    "output the runtime does not retain; run the check with command_run"),
    "receipt_missing": "no retained receipt is bound to the cited command",
    "ambiguous_ref": "more than one retained receipt carries the cited tool_call_ref",
    "call_incomplete": "the cited command did not complete (pending, interrupted, timed out or cancelled)",
    "nonzero_exit": "the cited command exited with a nonzero code",
    "zero_exit": "the cited command exited with code 0",
    "result_missing": "the cited command has no retained output record",
    "result_truncated": "the retained output of the cited command is truncated or inconsistent",
    "command_unrecorded": "the receipt of the cited command does not record its argv",
    "trivial_command": ("the cited command cannot verify anything (a no-op, print, listing, read or search "
                        "command such as pwd, ls, echo, cat or grep)"),
    "status_variable_unproven": ("the cited shell script ends with exit $VARIABLE, and the runtime could not prove "
                                 "that the variable still holds the exit status of a nontrivial command (only "
                                 "`CHECK...; NAME=$?; <print/read commands>; exit $NAME` in a plain sh/bash/dash -c "
                                 "script of literal words is recognized: no ${...}, $(...), glob or other expansion "
                                 "beyond a plain $NAME argument); run the check command directly instead"),
    "later_run_failed": "a later run of the same command in this attempt exited with a nonzero code",
    "later_run_unresolved": "a later run of the same command in this attempt has no established result",
}

COMMAND_OPERATIONS = frozenset({"command_run", "outside_command"})
# Gaps of a complete exit-0 run whose command cannot verify a pass.
TRIVIAL_GAPS = frozenset({"trivial_command", "status_variable_unproven"})

# Programs whose success says nothing about a named check: no-ops, printers,
# listings, file reads and searches. Matched on the argv[0] basename after
# the wrappers below are removed; a shell -c script is trivial only when
# every one of its commands is.
TRIVIAL_PROGRAMS = frozenset({
    ":", "[", "ag", "basename", "cat", "cd", "date", "df", "dirname", "du", "echo", "env",
    "export", "fd", "file", "find", "grep", "head", "hostname", "id", "less", "ls", "more",
    "printenv", "printf", "pwd", "readlink", "realpath", "rg", "set", "sleep", "sort", "stat",
    "tail", "tee", "test", "tr", "tree", "true", "type", "uname", "uniq", "wc", "which", "whoami", "yes",
})
# Read-only git subcommands; ``git diff`` counts only with a flag that makes
# its exit code meaningful.
TRIVIAL_GIT = frozenset({"branch", "config", "describe", "log", "ls-files", "remote", "rev-parse", "show", "status"})
GIT_DIFF_CHECK_FLAGS = frozenset({"--check", "--exit-code", "--quiet"})
SHELLS = frozenset({"sh", "bash", "dash", "zsh", "ksh"})
_SEPARATORS = frozenset({";", "&&", "||", "|", "&", "|&", ";;", "(", ")", "{", "}", "\n"})
_KEYWORDS = frozenset({"!", "if", "then", "else", "elif", "do", "while", "until", "fi", "done", "esac", "time"})
# The status-capture idiom (:func:`_status_capture_verifiable`) is proved
# only for these shells: zsh and ksh have lowercase special parameters (zsh
# `status` is `$?` itself) that a captured variable name could alias.
_CAPTURE_SHELLS = frozenset({"sh", "bash", "dash"})
# Tokens of syntax the idiom proof does not model (grouping, background,
# compound commands, negation, here-documents, arithmetic and process
# substitution); any of them keeps the script conservatively trivial.
_CAPTURE_SYNTAX = frozenset({"(", ")", "{", "}", "&", "|&", ";;", ";&", ";;&", "!", "[[", "]]", "if", "then",
                             "else", "elif", "fi", "do", "done", "while", "until", "for", "case", "esac",
                             "select", "time", "function", "coproc"})
# Builtins that can assign, make read-only, alias, trap or replace the shell,
# or run code the argv does not show.
_CAPTURE_STATE_BUILTINS = frozenset({".", "alias", "builtin", "command", "declare", "enable", "eval", "exec",
                                     "exit", "getopts", "let", "local", "mapfile", "read", "readarray",
                                     "readonly", "return", "shopt", "source", "trap", "typeset", "unset"})
# Commands allowed between `NAME=$?` and `exit $NAME`: printers and readers,
# none of which can change a shell variable. Bash printf can assign via %n
# or -v (including expanded options/formats), so exclude it entirely.
_CAPTURE_BETWEEN = TRIVIAL_PROGRAMS - {"cd", "env", "export", "printf", "set", "type"}


def _strip_wrappers(argv: list[str]) -> list[str]:
    """Drop leading assignments and the pass-through launchers env, timeout,
    nice, nohup, command, exec and time, which do not change what runs."""
    words = list(argv)
    while words:
        head = words[0].rsplit("/", 1)[-1]
        if "=" in words[0] and words[0].split("=", 1)[0].isidentifier():
            words = words[1:]
        elif head == "env":
            words = words[1:]
            while words and (words[0].startswith("-") or ("=" in words[0] and words[0].split("=", 1)[0].isidentifier())):
                words = words[2:] if words[0] in {"-u", "--unset", "-C", "--chdir"} else words[1:]
            if not words:
                return ["env"]
        elif head == "timeout":
            words = words[1:]
            while words and words[0].startswith("-"):
                words = words[2:] if words[0] in {"-s", "--signal", "-k", "--kill-after"} else words[1:]
            words = words[1:]  # the duration
        elif head == "nice":
            words = words[1:]
            while words and words[0].startswith("-"):
                words = words[2:] if words[0] in {"-n", "--adjustment"} else words[1:]
        elif head in {"nohup", "command", "exec", "time"}:
            words = words[1:]
            while words and words[0].startswith("-"):
                words = words[1:]
        else:
            break
    return words


def _shell_script(words: list[str]) -> tuple[str, str] | None:
    """``(options, script)`` of a ``sh``/``bash -c`` call, else ``None``."""
    options = ""
    for index, word in enumerate(words[1:], start=1):
        if word.startswith("-") and not word.startswith("--"):
            options += word[1:]
            if "c" in word[1:]:
                return (options, words[index + 1]) if index + 1 < len(words) else (options, "")
        elif word.startswith("--") or word.startswith("+"):
            continue
        else:
            return None
    return None


def _script_commands(script: str, *, preserve_quotes: bool = False) -> list[tuple[str, list[str]]] | None:
    """``[(separator before, words)]`` for each simple command of a script,
    redirections, command substitutions and control keywords removed;
    ``None`` when the script cannot be tokenized."""
    tokens = _script_tokens(script, preserve_quotes=preserve_quotes)
    if tokens is None:
        return None
    commands: list[tuple[str, list[str]]] = []
    separator, current, index = "", [], 0
    while index < len(tokens):
        token = tokens[index]
        if token == "$" and index + 1 < len(tokens) and tokens[index + 1] == "(":
            depth, index = 0, index + 1
            while index < len(tokens):  # a substitution is an argument, not a command
                depth += {"(": 1, ")": -1}.get(tokens[index], 0)
                if depth == 0:
                    break
                index += 1
            current.append("$(...)")
        elif token in _SEPARATORS:
            # The separator before the next command. An opening group keeps
            # the one before it; a line break after `&&`/`||`/`|` continues it.
            if current:
                commands.append((separator, current))
                current, separator = [], token
            elif token not in {"(", "{"} and not (token == "\n" and separator in {"&&", "||", "|"}):
                separator = token
        elif set(token) <= set("<>&|"):
            index += 1  # a redirection and its target
        elif token in _KEYWORDS and not current:
            pass
        else:
            current.append(token)
        index += 1
    if current:
        commands.append((separator, current))
    return commands


def _script_flags(options: str, commands) -> tuple[bool, bool]:
    """``(errexit, pipefail)`` set by the shell options or a ``set`` command."""
    errexit = "e" in options or any(words[0] == "set" and any(
        (w.startswith("-") and "e" in w[1:] and not w.startswith("--")) or w == "errexit" for w in words[1:])
        for _, words in commands)
    pipefail = any(words[0] == "set" and "pipefail" in words for _, words in commands)
    return errexit, pipefail


def _script_trivial(options: str, script: str, depth: int) -> bool:
    """A script is trivial unless a nontrivial command's exit status can
    reach the script's own exit status. Its last command decides; a trivial
    last command masks everything before it unless joined by ``&&`` (or by
    ``;``/newline under ``set -e``, or ``|`` under ``pipefail``)."""
    commands = _script_commands(script)
    if commands is None:
        return True
    return _commands_trivial(commands, *_script_flags(options, commands), depth)


def _commands_trivial(commands, errexit: bool, pipefail: bool, depth: int) -> bool:
    propagating = {"&&"} | ({";", "\n"} if errexit else set()) | ({"|"} if pipefail else set())
    commands = [(sep, words) for sep, words in commands if words[0] != "set"] or commands
    for separator, words in reversed(commands):
        if words[0] == "exit":
            # `exit`, `exit $?` and `|| exit N` (N nonzero) pass the previous
            # command's failure through; `exit 0` or a later exit masks it.
            if words[1:] in ([], ["$?"]) or (separator == "||" and words[1:] != ["0"]):
                continue
            return True
        if not trivial_command(words, depth + 1):
            return False
        if separator not in propagating:
            return True
    return True


def _script_tokens(script: str, *, preserve_quotes: bool = False) -> list[str] | None:
    import re
    import shlex

    if preserve_quotes:
        # Only for the capture proof's restricted grammar. Keep whole words,
        # including adjacent quoted parts; non-POSIX shlex splits those apart.
        if any(char in script for char in "\\`#"):
            return None
        pattern = re.compile(r"""[ \t\r]+|\n|[();<>|&]+|(?:[^ \t\r\n();<>|&'"]+|'[^']*'|"[^"]*")+""")
        tokens, at = [], 0
        while at < len(script):
            match = pattern.match(script, at)
            if match is None:
                return None
            token = match.group()
            if token == "\n" or not token.isspace():
                tokens.append(token)
            at = match.end()
        return tokens

    lexer = shlex.shlex(script.replace("\n", " \n "), posix=True, punctuation_chars=True)
    lexer.whitespace_split = True
    lexer.whitespace = " \t\r"
    try:
        return list(lexer)
    except ValueError:
        return None


def _status_variable(words) -> str | None:
    """``NAME`` of a final ``exit $NAME``/``exit ${NAME}``, else ``None``."""
    import re

    if len(words) != 2 or words[0] != "exit":
        return None
    match = re.fullmatch(r"\$(?:([A-Za-z_][A-Za-z0-9_]*)|\{([A-Za-z_][A-Za-z0-9_]*)\})", words[1])
    return (match.group(1) or match.group(2)) if match else None


def _status_capture_verifiable(words: list[str], options: str, script: str, depth: int) -> bool:
    """Whether a ``-c`` script provably exits with a nontrivial command's status
    through one captured variable::

        CHECK [> log 2>&1]; NAME=$?; [print/read commands;] exit $NAME

    Everything must hold: a ``sh``/``bash``/``dash`` script of simple commands
    only (no grouping, background, compound command, negation, here-document,
    command, arithmetic or process substitution, backslash, backtick or ``#``)
    whose every word is in the literal grammar of :func:`_capture_words_literal`
    (no parameter expansion beyond a plain ``$NAME``/``${NAME}`` argument, so no
    ``${...}`` operator, subscript or offset can assign); no assignment-prefixed
    command or leading numeric command word
    (including a file descriptor before a redirection); no builtin that can assign,
    make read-only, alias, trap or replace the shell (:data:`_CAPTURE_STATE_BUILTINS`);
    ``set`` only as leading ``-e``/``-u``/``-x``/``-o pipefail`` options and no
    ``+`` shell option; ``NAME`` a lowercase identifier other than ``_``,
    assigned exactly once, by the standalone word ``NAME=$?`` or ``NAME="$?"``
    (identifier and equals sign unquoted) that follows the
    commands before it after ``;`` or a newline; those commands, as a script
    of their own, are not trivial (:func:`_commands_trivial`), so ``$?`` is a
    nontrivial status; only :data:`_CAPTURE_BETWEEN` printers and readers run
    between the assignment and the final ``exit $NAME``, which follows them
    after ``;``, a newline or ``&&``. Anything else is not proved.
    """
    import re

    # The tokenizer treats `#` inside a word as a comment, unlike a shell.
    if _basename(words[0]) not in _CAPTURE_SHELLS or any(char in script for char in "\\`#"):
        return False
    for word in words[1:]:
        if word.startswith("+"):
            return False
        if word.startswith("-") and not word.startswith("--") and "c" in word[1:]:
            break
    tokens = _script_tokens(script)
    if tokens is None:
        return False
    def operator_paren(token):
        # Unquoted ( and ) are split into operator tokens; inside a quoted
        # argument (a `python3 -c "..."` program) they are plain text.
        return set(token) <= set("();<>|&") and ("(" in token or ")" in token)

    if any("$((" in token or "$[" in token for token in tokens):
        return False  # arithmetic expansion can assign, even inside quotes
    if any(token in _CAPTURE_SYNTAX or operator_paren(token) or token.startswith("<<") for token in tokens):
        return False
    commands = _script_commands(script)
    if not commands or len(commands) < 3:
        return False
    name = _status_variable(commands[-1][1])
    if name is None or not re.fullmatch(r"[a-z_][a-z0-9_]*", name) or name == "_":
        return False
    if commands[-1][0] not in {";", "\n", "&&"}:
        return False
    assignments = [i for i, (_, command) in enumerate(commands) if command == [f"{name}=$?"]]
    if len(assignments) != 1 or assignments[0] == 0 or commands[assignments[0]][0] not in {";", "\n"}:
        return False
    at = assignments[0]
    # Quote removal cannot prove assignment syntax: 'code=$?' is a command,
    # whereas code="$?" is an assignment. Bind the raw word to this exact
    # command, not merely to a matching substring elsewhere in the script.
    raw_commands = _script_commands(script, preserve_quotes=True)
    if (not raw_commands or len(raw_commands) != len(commands)
            or [sep for sep, _ in raw_commands] != [sep for sep, _ in commands]
            or raw_commands[at][1] not in ([f"{name}=$?"], [f'{name}="$?"'])
            or not _capture_words_literal(script, raw_commands[at][1][0])):
        return False
    leading = True
    for index, (_, command) in enumerate(commands[:-1]):
        # Prefixes can hide the actual command head: x=1 trap, x+=1 trap,
        # or 2>log x=1 trap (the tokenizer retains the descriptor as a word).
        # Fail closed instead of interpreting these stripped shell words.
        if (len(command) > 1 and "=" in command[0]) or command[0].isdecimal():
            return False
        head = _basename(command[0])
        if head in _CAPTURE_STATE_BUILTINS:
            return False
        if head == "set":
            if not leading or index >= at or not command[1:] or not all(
                    re.fullmatch(r"-[euxo]+|pipefail|errexit|nounset|xtrace", word) for word in command[1:]):
                return False
        else:
            leading = False
    prefix = commands[:at]
    if _commands_trivial(prefix, *_script_flags(options, prefix), depth):
        return False
    for _, command in commands[at + 1:-1]:
        if _basename(command[0]) not in _CAPTURE_BETWEEN:
            return False
    return True


def _capture_words_literal(script: str, capture: str) -> bool:
    """Whether every word of a status-capture script is in the proof's literal
    grammar, checked on the quote-preserving source words:

    - a command head (the first word after a separator) is literal: unquoted
      letters, digits and ``_./:,+@%=-``, single-quoted text, or double-quoted
      text without ``$``; the one exception is ``capture`` itself, exactly once;
    - a redirection target is literal too;
    - any other argument may also contain a plain ``$NAME`` or ``${NAME}``,
      unquoted or double-quoted, which expands without assigning anything.

    Everything else fails closed: ``${...}`` with any operator, subscript,
    offset, default or indirection; ``$1``, ``$@``, ``$$``, ``$'...'`` and
    ``$"..."``; command, arithmetic and process substitution; unquoted glob,
    brace and tilde characters; unbalanced quotes; unknown operators. Some
    harmless forms are rejected too; such a check is run directly instead.
    """
    import re

    tokens = _script_tokens(script, preserve_quotes=True)
    if tokens is None:
        return False
    # One alternative per character position (the identifier lookahead keeps
    # `$name` from being split), so a failed match cannot backtrack heavily.
    plain = r"[A-Za-z0-9_./:,+@%=-]"
    simple = r"\$(?:[A-Za-z_][A-Za-z0-9_]*(?![A-Za-z0-9_])|\{[A-Za-z_][A-Za-z0-9_]*\})"
    literal = re.compile(rf"""(?:{plain}|'[^']*'|"[^"$]*")+""")
    argument = re.compile(rf"""(?:{plain}|'[^']*'|{simple}|"(?:[^"$]|{simple})*")+""")
    head, captures, index = True, 0, 0
    while index < len(tokens):
        token = tokens[index]
        if token in {";", "\n", "&&", "||", "|"}:
            head = True
        elif set(token) <= set("();<>|&"):
            # Only a plain redirection, whose target is a literal file name.
            if (not set(token) <= set("<>&|") or not set(token) & set("<>") or token.startswith("<<")
                    or index + 1 >= len(tokens) or not literal.fullmatch(tokens[index + 1])):
                return False
            index += 1
        elif head and token == capture:
            captures, head = captures + 1, False
        elif (literal if head else argument).fullmatch(token):
            head = False
        else:
            return False
        index += 1
    return captures == 1


def _basename(word: str) -> str:
    return word.rsplit("/", 1)[-1]


def trivial_reason(argv) -> str | None:
    """The :data:`EVIDENCE_GAPS` code that keeps a successful ``argv`` from
    verifying a check: ``status_variable_unproven`` for a shell script that
    ends with ``exit $VARIABLE`` the runtime could not prove
    (:func:`_status_capture_verifiable`), ``trivial_command`` for any other
    trivial command (:func:`trivial_command`), else ``None``."""
    if not trivial_command(argv):
        return None
    words = _strip_wrappers(list(argv))
    parsed = _shell_script(words) if words and _basename(words[0]) in SHELLS else None
    commands = _script_commands(parsed[1]) if parsed else None
    if commands and _status_variable(commands[-1][1]) not in {None, "?"}:
        return "status_variable_unproven"
    return "trivial_command"


def trivial_command(argv, _depth: int = 0) -> bool:
    """True when ``argv`` cannot verify any named check.

    The matching rule (documented in docs/features/orchestration/index.md): after
    leading ``NAME=value`` assignments and the launchers ``env``,
    ``timeout``, ``nice``, ``nohup``, ``command``, ``exec`` and ``time`` are
    removed, a command is trivial when nothing is left; when its program
    basename is in :data:`TRIVIAL_PROGRAMS`; when it is a read-only ``git``
    subcommand (:data:`TRIVIAL_GIT`, or ``git diff`` without ``--check``,
    ``--exit-code`` or ``--quiet``); or when it is a ``sh``/``bash -c``
    script that is trivial by :func:`_script_trivial` (an unparsable script
    counts as trivial) and not the narrowly proved status-capture idiom of
    :func:`_status_capture_verifiable`. Anything else -- a test runner,
    linter, type checker, build or project script -- may verify a check.
    Whether it verifies *the named* check is the independent reviewer's
    judgement.
    """
    if not isinstance(argv, (list, tuple)) or not argv or not all(isinstance(w, str) for w in argv):
        return True
    words = _strip_wrappers(list(argv))
    if not words or not words[0]:
        return True
    program = words[0].rsplit("/", 1)[-1]
    if program in TRIVIAL_PROGRAMS or program in {"exit", "return"}:
        return True
    if program == "git":
        rest = words[1:]
        while rest and rest[0].startswith("-"):
            rest = rest[2:] if rest[0] in {"-C", "-c", "--git-dir", "--work-tree"} else rest[1:]
        if not rest:
            return True
        if rest[0] == "diff":
            return not (GIT_DIFF_CHECK_FLAGS & set(rest[1:]))
        return rest[0] in TRIVIAL_GIT
    if program in SHELLS:
        parsed = _shell_script(words)
        if parsed is None:
            return False  # a script file: its content is the project's own check
        if _depth >= 4:
            return True
        return (_script_trivial(parsed[0], parsed[1], _depth)
                and not _status_capture_verifiable(words, parsed[0], parsed[1], _depth))
    return False


def _gap(gap: str, **detail) -> dict:
    return dict(strength="weak", gap=gap, **detail)


def _engine_assessment(db, task_id: str, attempt_id: str, ref: str) -> dict:
    rows = db.execute(
        "SELECT attempt_id, kind, status FROM tool_result_observations WHERE task_id=? AND tool_call_ref=? ORDER BY id",
        (task_id, ref),
    ).fetchall()
    own = [row for row in rows if row["attempt_id"] == attempt_id]
    if not own:
        return _gap("foreign_attempt" if rows else "tool_call_ref_unknown")
    kind = next((row["kind"] for row in own if row["kind"]), None)
    if kind != "execute":
        return _gap("not_a_command", operation=kind)
    if own[-1]["status"] not in {"completed", "failed"}:
        return _gap("call_incomplete", operation="engine_execute")
    return _gap("engine_command_unverifiable", operation="engine_execute")


def _gateway_assessment(db, task_id: str, attempt_id: str, ref: str, outcome: str) -> dict:
    import json

    authorized = [
        (row["id"], row["actor"], json.loads(row["payload_json"]))
        for row in db.execute(
            "SELECT id, actor, payload_json FROM runtime_events WHERE task_id=? AND type='TOOL_AUTHORIZED' "
            "AND instr(payload_json, ?) > 0", (task_id, ref),
        )
    ]
    authorized = [(order, actor, payload) for order, actor, payload in authorized if payload.get("tool_call_ref") == ref]
    if not authorized:
        return _gap("tool_call_ref_unknown")
    if len(authorized) != 1:
        return _gap("ambiguous_ref")
    order, actor, payload = authorized[0]
    operation = payload.get("operation")
    channel = db.execute("SELECT attempt_id FROM trusted_channels WHERE channel_id=?", (actor,)).fetchone()
    if channel is None or channel["attempt_id"] != attempt_id:
        return _gap("foreign_attempt", operation=operation)
    if operation not in COMMAND_OPERATIONS:
        return _gap("not_a_command", operation=operation)
    receipts = []
    pending = False
    for row in db.execute(
        "SELECT result_json FROM runtime_requests WHERE task_id=? AND actor=? AND operation=? AND instr(result_json, ?) > 0",
        (task_id, actor, operation, ref),
    ):
        saved = json.loads(row["result_json"])
        receipt = saved.get("receipt") if isinstance(saved.get("receipt"), dict) else {}
        if receipt.get("tool_call_ref") == ref:
            receipts.append((saved, receipt))
        elif saved.get("tool_call_ref") == ref and not saved.get("complete"):
            pending = True
    if len(receipts) > 1 or (receipts and pending):
        return _gap("ambiguous_ref", operation=operation)
    if not receipts:
        return _gap("call_incomplete" if pending else "receipt_missing", operation=operation)
    saved, receipt = receipts[0]
    exit_code = receipt.get("exit_code")
    identity_receipt = receipt
    if operation == "command_run" and "cwd" not in receipt:
        from .. import task_workspace
        identity_receipt = dict(receipt, cwd=task_workspace.task_root(db, task_id), profile="restricted")
    detail = dict(operation=operation, exit_code=exit_code if type(exit_code) is int else None,
                  order=order, identity=command_identity(operation, identity_receipt))
    if (saved.get("complete") is not True or receipt.get("complete") is not True
            or receipt.get("failure") is not None or type(exit_code) is not int):
        return _gap("call_incomplete", **detail)
    if 'runtime_verification' in receipt:
        from ..orchestration.reviewer_verification import validate_reuse, observation
        if not validate_reuse(db, task_id, attempt_id, receipt):
            return _gap("result_missing", **detail)
        source = receipt['runtime_verification']['command_ref']
        detail.update(evidence_source='runtime_verification', runtime_command_ref=source,
                      order=observation(db, task_id, source)['started_event_id'])
    if outcome == "passed" and exit_code != 0:
        return _gap("nonzero_exit", **detail)
    if outcome == "failed" and exit_code == 0:
        return _gap("zero_exit", **detail)
    output_id = receipt.get("large_output_id")
    retained = db.execute(
        "SELECT l.source_ref, l.truncated, l.exit_code, l.failure, l.byte_count, l.owner_scope, p.attempt_id "
        "FROM large_outputs l LEFT JOIN delegation_provisioning p "
        "ON p.task_id=l.task_id AND p.idempotency_key=l.owner_scope WHERE l.id=? AND l.task_id=?",
        (output_id, task_id),
    ).fetchone() if isinstance(output_id, str) and output_id else None
    if retained is None or retained["source_ref"] != ref or retained["attempt_id"] != attempt_id:
        return _gap("result_missing", **detail)
    if (retained["truncated"] or retained["exit_code"] != exit_code or retained["failure"] is not None
            or type(retained["byte_count"]) is not int):
        return _gap("result_truncated", **detail)
    argv = receipt.get("argv")
    if not isinstance(argv, list) or not argv:
        return _gap("command_unrecorded", **detail)
    trivial = trivial_reason(argv) if outcome == "passed" else None
    if trivial:
        return _gap(trivial, **detail)
    detail["output_id"] = output_id
    if outcome == "failed" and "runtime_verification" not in receipt:
        detail["invocation_error"] = _probe_invocation_error(argv, receipt.get("output"), retained["byte_count"])
    return dict(strength="verified", gap=None, **detail)


def _probe_invocation_error(argv, output, byte_count):
    """Source-backed mistakes in standalone python/python3 -B/-I/-S -c probes.

    Cover syntax errors (nothing executed), one corrected regex literal with
    every assertion/product call unchanged, and literal-data expectation errors.
    The latter permits only single-assignment data bindings and equality asserts
    over literals, earlier bindings and unary len/sorted/sum/min/max/all/any/abs.
    Exactly one expected literal may change; statically established false/true
    comparisons plus a recorded <string> AssertionError prove a probe mistake
    without executing submitted source or weakening a product assertion.

    Project test/lint/build commands are never eligible: no script/module runner,
    imported project code, IO, subprocess, dynamic execution or arbitrary calls
    are allowed in the new literal-data shape. Existing regex probes still reject
    pytest/unittest imports. Runtime-owned verification is excluded by assessment
    and _invocation_retraction, regardless of probe shape. Other languages and
    unprovable probes fail closed, rather than treating a claim as evidence.
    """
    import ast
    import re
    from pathlib import Path

    if (not isinstance(output, str) or len(output.encode("utf-8")) != byte_count
            or Path(argv[0]).name not in {"python", "python3"}
            or "-c" not in argv or argv.index("-c") != len(argv) - 2
            or any(word not in {"-B", "-I", "-S"} for word in argv[1:argv.index("-c")])):
        return None
    source = argv[-1]
    if len(source) > 8192:
        return None
    try:
        tree = ast.parse(source)
    except SyntaxError:
        return "python_syntax" if re.search(r"(?m)^SyntaxError: [^\n]+\Z", output.rstrip()) else None
    imports = [node for node in ast.walk(tree) if isinstance(node, (ast.Import, ast.ImportFrom))]
    if any((isinstance(node, ast.ImportFrom) and (node.module or "").split(".")[0] in {"pytest", "unittest"})
           or (isinstance(node, ast.Import) and any(alias.name.split(".")[0] in {"pytest", "unittest"}
                                                   for alias in node.names)) for node in imports):
        return None
    if ('File "<string>"' in output
            and re.search(r"(?m)^AssertionError(?::[^\n]*)?\Z", output.rstrip())):
        expectations = _literal_expectations(tree)
        if expectations and any(not holds for _, holds in expectations):
            return "python_expectation"
    if not any(isinstance(node, ast.Import) and any(alias.name == "re" and not alias.asname
                                                   for alias in node.names) for node in imports):
        return None
    if not ('File "<string>"' in output
            and re.search(r"(?m)^(?:re\.(?:error|PatternError)|AssertionError):[^\n]*\Z", output.rstrip())):
        return None
    return "python_regex" if _regex_literals(tree) else None


def _literal_expectations(tree):
    """Statically check a bounded, closed literal-data probe; never eval/exec it."""
    import ast

    if len(list(ast.walk(tree))) > 256:
        return None
    functions = dict(len=len, sorted=sorted, sum=sum, min=min, max=max, all=all, any=any, abs=abs)
    bindings, assertions = {}, []

    def value(node):
        if isinstance(node, ast.Name) and node.id in bindings:
            return bindings[node.id]
        if (isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
                and node.func.id in functions and len(node.args) == 1 and not node.keywords):
            return functions[node.func.id](value(node.args[0]))
        return ast.literal_eval(node)

    try:
        for node in tree.body:
            if (isinstance(node, ast.Assign) and len(node.targets) == 1
                    and isinstance(node.targets[0], ast.Name)):
                name = node.targets[0].id
                if name in bindings or name in functions:
                    return None
                bindings[name] = value(node.value)
            elif (isinstance(node, ast.Assert) and isinstance(node.test, ast.Compare)
                    and len(node.test.ops) == 1 and isinstance(node.test.ops[0], ast.Eq)):
                expected = ast.literal_eval(node.test.comparators[0])
                if node.msg is not None:
                    value(node.msg)
                assertions.append((node, value(node.test.left) == expected))
            else:
                return None
    except (ValueError, TypeError, KeyError, OverflowError, RecursionError):
        return None
    return assertions or None


def _regex_literals(tree):
    import ast

    return [node.args[0] for node in ast.walk(tree)
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
            and isinstance(node.func.value, ast.Name) and node.func.value.id == "re"
            and node.func.attr in {"compile", "search", "match", "fullmatch", "findall", "finditer", "split", "sub", "subn"}
            and node.args and isinstance(node.args[0], ast.Constant) and isinstance(node.args[0].value, str)]


def _probe_correction(failed_argv, passed_argv, kind):
    """Preserve probe structure; change one regex or proven literal expectation."""
    import ast

    if failed_argv[:-1] != passed_argv[:-1] or len(passed_argv[-1]) > 8192:
        return False
    try:
        corrected = ast.parse(passed_argv[-1])
    except (SyntaxError, ValueError):
        return False
    if kind == "python_syntax":
        return True  # The original script could not execute at all.
    original = ast.parse(failed_argv[-1])
    if kind == "python_expectation":
        before, after = _literal_expectations(original), _literal_expectations(corrected)
        if not before or not after or len(before) != len(after) or not all(holds for _, holds in after):
            return False
        changed = [(left, right) for (left, _), (right, _) in zip(before, after)
                   if ast.dump(left.test.comparators[0]) != ast.dump(right.test.comparators[0])]
        if len(changed) != 1 or [holds for _, holds in before].count(False) != 1:
            return False
        left, right = changed[0]
        if next(holds for node, holds in before if node is left):
            return False
        left.test.comparators[0] = right.test.comparators[0]
        return ast.dump(original) == ast.dump(corrected)
    before, after = _regex_literals(original), _regex_literals(corrected)
    if len(before) != len(after):
        return False
    changed = [(left, right) for left, right in zip(before, after) if left.value != right.value]
    if len(changed) != 1:
        return False
    left, right = changed[0]
    # AST equality ignores formatting/comments, never assertions or test targets.
    left.value = right.value
    return ast.dump(original) == ast.dump(corrected)


def command_identity(operation, receipt) -> tuple | None:
    """The identity of one command run: its operation, exact argv and
    working directory and effective command profile. Legacy ``command_run``
    receipts without cwd are mapped to the task root by gateway assessment;
    they can never close an outside-cwd or Development run. ``None`` when the receipt does not record
    the argv or (for ``outside_command``) the cwd: such a run cannot be
    matched to any other. _same_command permits exact argv/cwd closure across
    the two approved operations without erasing their profile provenance."""
    argv = receipt.get("argv") if isinstance(receipt, dict) else None
    if not isinstance(argv, list) or not argv or not all(isinstance(word, str) for word in argv):
        return None
    if operation == "command_run":
        return (operation, tuple(argv), receipt.get("cwd"), receipt.get("profile", "restricted"))
    cwd = receipt.get("cwd")
    if operation == "outside_command" and isinstance(cwd, str) and cwd:
        return (operation, tuple(argv), cwd, receipt.get("profile", "outside_confirmed"))
    return None


def _same_command(left, right):
    """Exact argv/cwd across approved command routes; otherwise retain profiles.

    Keep operation/profile provenance in identities and receipts. Switching to or
    from outside_command necessarily changes the profile (outside_confirmed), so
    that approved route alone may close the identical argv at the identical cwd.
    Same-route profile changes and missing cwd never establish this exception.
    """
    if left is None or right is None:
        return False
    if left == right:
        return True
    return (left[0] in COMMAND_OPERATIONS and right[0] in COMMAND_OPERATIONS
            and left[0] != right[0] and isinstance(left[2], str) and bool(left[2])
            and left[1:3] == right[1:3])


def assess_check(db, task_id: str, attempt_id: str, outcome: str, tool_call_ref) -> dict:
    """Assess one check's cited evidence inside an open connection.

    Returns ``dict(strength, gap, operation?, exit_code?, order?, identity?)``
    (``order`` is the run's authorization event id, or the original runtime
    execution-start event id for a reused verification observation, and ``identity`` its
    :func:`command_identity`, once a receipt is bound): ``strength`` is
    ``verified``, ``weak`` (with the :data:`EVIDENCE_GAPS` code in ``gap``)
    or ``none`` for a ``not_run``/``unverified`` disclosure. Only a ``gateway:``
    receipt from this exact task and attempt can verify a check.
    """
    if outcome not in {"passed", "failed"}:
        return dict(strength="none", gap=None)
    if not isinstance(tool_call_ref, str) or not tool_call_ref:
        return _gap("tool_call_ref_missing")
    if tool_call_ref.startswith("gateway:"):
        return _gateway_assessment(db, task_id, attempt_id, tool_call_ref, outcome)
    return _engine_assessment(db, task_id, attempt_id, tool_call_ref)


def assess_checks(db, task_id: str, attempt_id: str, checks) -> list[dict]:
    """:func:`assess_check` over one result's checks, in order. A
    ``tool_call_ref`` already cited by an earlier check of the same result
    is a duplicated binding and verifies nothing again."""
    cited: set[str] = set()
    assessments = []
    for check in checks or ():
        ref = check.get("tool_call_ref") if isinstance(check, dict) else None
        outcome = check.get("outcome") if isinstance(check, dict) else None
        if outcome in {"passed", "failed"} and isinstance(ref, str) and ref in cited:
            assessments.append(_gap("ref_already_cited"))
        else:
            assessed = assess_check(db, task_id, attempt_id, outcome, ref)
            source = assessed.get('runtime_command_ref')
            if source and source in cited:
                assessed = _gap("ref_already_cited")
            elif source:
                cited.add(source)
            assessments.append(assessed)
        if isinstance(ref, str) and ref:
            cited.add(ref)
    return assessments


def command_history(db, task_id: str, attempt_id: str) -> list[dict]:
    """Every gateway command observation of this attempt, in execution order.

    Each entry is ``dict(ref, order, identity, status)``: ``status`` is
    ``passed`` (complete, exit 0, complete retained output), ``failed``
    (complete, nonzero exit, complete retained output) or ``unknown``
    (pending, interrupted, timed out, unretained or inconsistent);
    ``identity`` is :func:`command_identity`, ``None`` when unrecorded.
    A run the gateway rejected before executing it is not history.
    Nothing here judges whether a command is trivial: that decides whether
    a pass verifies a check, not whether the run happened.
    """
    import json

    channels = [row["channel_id"] for row in db.execute(
        "SELECT channel_id FROM trusted_channels WHERE task_id=? AND attempt_id=?", (task_id, attempt_id))]
    if not channels:
        return []
    marks = ",".join("?" * len(channels))
    # A run the gateway refused before any side effect never happened.
    rejected = {json.loads(row["payload_json"]).get("tool_call_ref") for row in db.execute(
        f"SELECT payload_json FROM runtime_events WHERE task_id=? AND type='TOOL_REJECTED' AND actor IN ({marks})",
        (task_id, *channels))}
    history = []
    for row in db.execute(
        f"SELECT id, payload_json FROM runtime_events WHERE task_id=? AND type='TOOL_AUTHORIZED' "
        f"AND actor IN ({marks}) ORDER BY id", (task_id, *channels),
    ):
        payload = json.loads(row["payload_json"])
        ref = payload.get("tool_call_ref")
        if payload.get("operation") not in COMMAND_OPERATIONS or not isinstance(ref, str) or ref in rejected:
            continue
        passed = _gateway_assessment(db, task_id, attempt_id, ref, "passed")
        if passed["strength"] == "verified" or passed["gap"] in TRIVIAL_GAPS:
            status = "passed"
        elif passed["gap"] == "nonzero_exit":
            failed = _gateway_assessment(db, task_id, attempt_id, ref, "failed")
            status = "failed" if failed["strength"] == "verified" else "unknown"
        else:
            status = "unknown"
        history.append(dict(ref=ref, order=passed.get('order', row['id']), identity=passed.get("identity"), status=status))
    return sorted(history, key=lambda run: run['order'])


def current_outcomes(db, task_id: str, attempt_id: str, checks) -> list[dict]:
    """Each check's current outcome, separated from the attempt's run history.

    Returns one ``dict(outcome, gap, closed_by)`` per check, in order:

    - A ``passed`` claim is ``passed`` only when :func:`assess_checks`
      verifies it *and* no later run of the same command identity
      (:func:`command_identity`, compared by :func:`_same_command`) in this
      attempt has a different result: a
      later failing run makes it ``failed`` (``later_run_failed``), a later
      run without a known result makes it ``unverified``
      (``later_run_unresolved``). Otherwise an unverifiable pass is
      ``failed`` for a nonzero exit, else ``unverified``, with its gap.
    - A ``failed`` claim stays ``failed`` unless its cited run is a
      verified failure *and* a ``passed`` check of this same result cites a
      verified, later matching run (exact argv/cwd across command operations,
      same profile within one operation) that is the latest matching run in
      this attempt, with no later command run whose
      identity is unrecorded. Only then is it ``passed`` with ``closed_by``
      naming that closing ref. The claim itself is never rewritten: the
      failed check stays in the result as history.
    - An explicit, validated own-probe invocation error is ``retracted``,
      never passed, with ``retracted_by`` naming the later observed correction.
      Both checks and the nonzero receipt remain history.
    - ``not_run`` and ``unverified`` are kept as claimed.

    Ambiguity never closes a failure; nothing here turns a current failure
    into a warning.
    """
    checks = [check if isinstance(check, dict) else {} for check in checks or ()]
    assessments = assess_checks(db, task_id, attempt_id, checks)
    history = None
    outcomes = []
    for check, assessment in zip(checks, assessments):
        outcome = check.get("outcome")
        if outcome not in {"passed", "failed"}:
            outcomes.append(dict(outcome=outcome, gap=None, closed_by=None))
            continue
        if assessment["strength"] == "verified" and assessment.get("identity") is None:
            # A verified run whose receipt records no identity (an older
            # outside_command receipt without cwd) matches no other run: a
            # pass keeps its own verdict, a failure can never be closed.
            outcomes.append(dict(outcome=outcome, gap=None, closed_by=None))
            continue
        if assessment["strength"] != "verified":
            if outcome == "passed":
                outcomes.append(dict(outcome="failed" if assessment["gap"] == "nonzero_exit" else "unverified",
                                     gap=assessment["gap"], closed_by=None))
            else:
                outcomes.append(dict(outcome="failed", gap=assessment["gap"], closed_by=None))
            continue
        if history is None:
            history = command_history(db, task_id, attempt_id)
        same = [run for run in history if _same_command(run["identity"], assessment["identity"])]
        latest = same[-1] if same else None
        if outcome == "passed":
            if latest is None or latest["order"] <= assessment["order"] or latest["status"] == "passed":
                outcomes.append(dict(outcome="passed", gap=None, closed_by=None))
            elif latest["status"] == "failed":
                outcomes.append(dict(outcome="failed", gap="later_run_failed", closed_by=None))
            else:
                outcomes.append(dict(outcome="unverified", gap="later_run_unresolved", closed_by=None))
            continue
        closing = None
        if latest is not None and latest["status"] == "passed" and latest["order"] > assessment["order"]:
            unrecorded_later = any(run["identity"] is None and run["order"] > assessment["order"] for run in history)
            cited = [index for index, other in enumerate(checks)
                     if other.get("outcome") == "passed" and other.get("tool_call_ref") == latest["ref"]
                     and assessments[index]["strength"] == "verified"]
            if not unrecorded_later and len(cited) == 1:
                closing = latest["ref"]
        outcomes.append(dict(outcome="passed", gap=None, closed_by=closing) if closing
                        else dict(outcome="failed", gap=None, closed_by=None))
    # Explicit retraction is not a pass and never changes the failed receipt.
    # Derive it only after the corrected check's ordinary current outcome.
    for index, check in enumerate(checks):
        if not check.get("invocation_error"):
            continue
        if history is None:
            history = command_history(db, task_id, attempt_id)
        ref = _invocation_retraction(db, task_id, index, checks, assessments, outcomes, history)
        if ref:
            outcomes[index] = dict(outcome="retracted", gap=None, closed_by=None, retracted_by=ref)
    # A worker's own exploratory probe (an expected 404, a throwaway
    # diagnostic) withdrawn with its reason: retracted history, never passed.
    # One cited 404 probe once turned an approving review into a failed
    # assignment that could not be submitted (2026-10-22).
    for index, check in enumerate(checks):
        reason = check.get("probe_withdrawn")
        if (isinstance(reason, str) and reason.strip() and check.get("outcome") == "failed"
                and outcomes[index]["outcome"] == "failed" and assessments[index]["strength"] == "verified"):
            outcomes[index] = dict(outcome="retracted", gap=None, closed_by=None, retracted_by="probe_withdrawn")
    return outcomes


def _invocation_retraction(db, task_id, index, checks, assessments, outcomes, history):
    """Retract only eligible own probes, never project test/lint/build commands
    or evidence_source == runtime_verification, even with explicit model claims.
    Preserve unique binding, operation/cwd/profile, output and ordering gates.
    """
    check, failed = checks[index], assessments[index]
    request = check.get("invocation_error")
    if (check.get("outcome") != "failed" or not isinstance(request, dict)
            or not isinstance(request.get("reason"), str) or not request["reason"].strip()
            or failed["strength"] != "verified" or not failed.get("invocation_error")
            or failed.get("evidence_source") == "runtime_verification" or failed.get("identity") is None):
        return None
    ref = request.get("corrected_by")
    if "retracted_by" in check and check["retracted_by"] != ref:
        return None
    matches = [i for i, other in enumerate(checks)
               if other.get("outcome") == "passed" and other.get("tool_call_ref") == ref
               and other.get("name") == check.get("name")]
    if len(matches) != 1 or sum(
            other.get("invocation_error", {}).get("corrected_by") == ref
            for other in checks if isinstance(other.get("invocation_error"), dict)) != 1:
        return None
    target = matches[0]
    passed = assessments[target]
    if (passed["strength"] != "verified" or outcomes[target]["outcome"] != "passed"
            or passed.get("evidence_source") == "runtime_verification"
            or passed.get("order", 0) <= failed["order"] or passed.get("identity") is None
            or passed["identity"][0] != failed["identity"][0]
            or passed["identity"][2:] != failed["identity"][2:]
            or not _probe_correction(failed["identity"][1], passed["identity"][1], failed["invocation_error"])):
        return None
    from ..orchestration.check_closure import _retained_output_complete

    for assessed in (failed, passed):
        output = db.execute("SELECT * FROM large_outputs WHERE task_id=? AND id=?",
                            (task_id, assessed.get("output_id"))).fetchone()
        if output is None or not _retained_output_complete(db, task_id, output):
            return None
    # Neither an unresolved run nor another later failure can be explained
    # away by correcting this particular invocation.
    if any(run["order"] > failed["order"] and (
            run["identity"] is None or
            (_same_command(run["identity"], failed["identity"]) and run["status"] != "passed"))
           for run in history):
        return None
    return ref


def accept_checks(db, task_id: str, attempt_id: str, checks) -> list[dict]:
    """The checks the runtime accepts from one ``result_submit``
    (:func:`current_outcomes` decides).

    A verified, still-current ``passed`` stays passed. Any other ``passed``
    claim is recorded ``failed`` when its cited command -- or a later run of
    the same command -- exited nonzero (an observed failure is never
    softened), else ``unverified`` with the runtime's blocker; both keep
    ``claimed_outcome='passed'`` and the structured ``evidence_gap`` code.
    A ``failed`` check stays ``failed`` in the record, as history; when an
    exact later rerun closes it, it carries ``closed_by`` with the closing
    ``tool_call_ref``. A validated invocation error instead carries
    ``retracted_by`` and its stated reason, while retaining ``outcome=failed``.
    ``not_run`` and ``unverified`` are kept as claimed.
    """
    from ..contracts import StoreError

    accepted = []
    for index, (check, current) in enumerate(zip(checks, current_outcomes(db, task_id, attempt_id, checks))):
        if "invocation_error" in check and current["outcome"] != "retracted":
            raise StoreError("invalid_check_retraction", f"arguments.checks[{index}].invocation_error",
                             "not bound to an eligible own invocation error and later observed correction",
                             "a verified failed standalone Python syntax/regex/literal-expectation probe and a later verified passing correction (only one regex or proven expected literal changes)",
                             "Retain real failures and use an exact rerun. Retract only your own supported probe invocation error, "
                             "with a nonempty reason and corrected_by naming a passed check of this result with the same name, "
                             "operation, cwd and profile. Claimed, incomplete, foreign and runtime verification checks cannot be retracted.")
        if check["outcome"] == "failed":
            if current["outcome"] == "retracted":
                accepted.append(dict(check, retracted_by=current["retracted_by"]))
            else:
                accepted.append(dict(check, closed_by=current["closed_by"]) if current["closed_by"] else dict(check))
            continue
        if check["outcome"] != "passed" or current["outcome"] == "passed":
            accepted.append(dict(check))
            continue
        entry = {key: check[key] for key in ("name", "tool_call_ref") if check.get(key)}
        if current["outcome"] == "failed":
            entry["outcome"] = "failed"
        else:
            entry["outcome"] = "unverified"
            entry["blocker"] = "Runtime kept this passed claim unverified: " + EVIDENCE_GAPS[current["gap"]] + "."
        entry.update(claimed_outcome="passed", evidence_gap=current["gap"])
        accepted.append(entry)
    return accepted


def open_failure(db, task_id: str, attempt_id: str, checks) -> bool:
    """Whether a recorded result still carries a current failure: a
    ``failed`` check that no exact later rerun closes (a stored
    ``closed_by`` counts only when it equals the closing ref derived now),
    a retraction that cannot be rederived from its original and correction
    receipts, or a ``passed`` check that is not currently verified. ``not_run`` and
    ``unverified`` are disclosures, never failures."""
    checks = [check if isinstance(check, dict) else {} for check in checks or ()]
    if omitted_failures(db, task_id, attempt_id, checks):
        return True
    for check, current in zip(checks, current_outcomes(db, task_id, attempt_id, checks)):
        if check.get("outcome") == "failed" and not (
                (current["closed_by"] and check.get("closed_by") == current["closed_by"])
                or (current["outcome"] == "retracted" and check.get("retracted_by") == current["retracted_by"])):
            return True
        if check.get("outcome") == "passed" and current["outcome"] != "passed":
            return True
    return False


# One runtime event per failed command receipt a result_submit of the attempt
# presented (accepted or refused), so a later submission cannot drop it.
PRESENTED_FAILURE_EVENT = "CHECK_FAILURE_PRESENTED"


def _attempt_channels(db, task_id: str, attempt_id: str) -> list[str]:
    return [row["channel_id"] for row in db.execute(
        "SELECT channel_id FROM trusted_channels WHERE task_id=? AND attempt_id=?", (task_id, attempt_id))]


def presented_failures(db, task_id: str, attempt_id: str) -> list[str]:
    """Every gateway ``tool_call_ref`` a ``result_submit`` of this attempt
    cited as a passed or failed check while its receipt was a verified
    failure or not yet complete, in first-presented order."""
    import json

    channels = _attempt_channels(db, task_id, attempt_id)
    if not channels:
        return []
    marks = ",".join("?" * len(channels))
    refs: list[str] = []
    for row in db.execute(
            f"SELECT payload_json FROM runtime_events WHERE task_id=? AND type=? AND actor IN ({marks}) ORDER BY id",
            (task_id, PRESENTED_FAILURE_EVENT, *channels)):
        payload = json.loads(row["payload_json"])
        ref = payload.get("tool_call_ref")
        if payload.get("attempt_id") == attempt_id and isinstance(ref, str) and ref not in refs:
            refs.append(ref)
    return refs


def record_presented_failures(db, task_id: str, attempt_id: str, actor: str, checks) -> list[str]:
    """Retain, inside the caller's transaction, each gateway receipt that
    ``checks`` cite with outcome passed or failed and that is a verified
    failed run of this attempt (complete, nonzero exit, complete retained
    output) or a run of this attempt without a result yet. Runs never cited
    as a check (an incidental search that exited 1) are not retained;
    foreign, unknown and non-command references are not either. Idempotent.
    Returns the refs newly retained."""
    from ..orchestration.state import event

    known = set(presented_failures(db, task_id, attempt_id))
    added = []
    for check in checks or ():
        ref = check.get("tool_call_ref") if isinstance(check, dict) else None
        if (not isinstance(check, dict) or check.get("outcome") not in {"passed", "failed"}
                or not isinstance(ref, str) or not ref.startswith("gateway:") or ref in known):
            continue
        assessed = _gateway_assessment(db, task_id, attempt_id, ref, "failed")
        if assessed["strength"] == "verified" or assessed["gap"] == "call_incomplete":
            event(db, task_id, PRESENTED_FAILURE_EVENT, dict(attempt_id=attempt_id, tool_call_ref=ref), actor)
            known.add(ref)
            added.append(ref)
    return added


def omitted_failures(db, task_id: str, attempt_id: str, checks) -> list[str]:
    """Retained presented receipts (:func:`presented_failures`) that are now
    verified failed runs but that no ``passed`` or ``failed`` check of
    ``checks`` cites again. A failure, once presented, stays part of the
    attempt: it must be listed again, closed, retracted or reported in an
    explicitly failed assignment, never dropped or relabelled."""
    presented = presented_failures(db, task_id, attempt_id)
    if not presented:
        return []
    cited = {check.get("tool_call_ref") for check in checks or ()
             if isinstance(check, dict) and check.get("outcome") in {"passed", "failed"}}
    return [ref for ref in presented if ref not in cited
            and _gateway_assessment(db, task_id, attempt_id, ref, "failed")["strength"] == "verified"]


def _identity_label(identity) -> str:
    """A bounded description of a command identity that never repeats the
    argv itself (a shell script can hold anything): program, word count and
    an argv digest, plus cwd and profile."""
    import json

    operation, argv, cwd, profile = identity
    digest = hashlib.sha256(json.dumps(list(argv), ensure_ascii=False, separators=(",", ":")).encode()).hexdigest()
    cwd_text = cwd if isinstance(cwd, str) and len(cwd) <= 160 else f"{len(cwd or '')} characters"
    return (f"{operation} {_basename(argv[0])[:40]} ({len(argv)} argv words, argv_sha256 {digest[:16]}, "
            f"cwd {cwd_text}, profile {profile})")


_EXACT_RERUN = ("Rerun exactly the same argv and cwd (same profile on the same route; command_run and outside_command "
                "may close each other) with a new request_key, keep this failed "
                "check, and add a passed check citing the new receipt.")
_FAIL_ASSIGNMENT = ('Otherwise send assignment_outcome="failed" with this check retained to submit an intentional '
                    "failed assignment.")
_DIRECT = ("Run checks directly as argv (for example the test runner itself), not wrapped in a shell script that "
           "redirects or captures the status: the gateway retains the complete output whatever its size, a passed "
           "check is verified from that retained output, and read_large_output pages it.")


def explain_open_failure(db, task_id: str, attempt_id: str, checks, assessments, index: int) -> tuple[str, str]:
    """``(diagnosis, correction)`` for the open failure ``checks[index]`` of
    an accepted result (:func:`accept_checks`). The diagnosis names the
    failed receipt, a bounded command identity (:func:`_identity_label`), the
    relevant later rerun and its actual evidence gap; the correction is one
    that can close this identity, or explicitly failing the assignment when
    nothing can. Never the argv, a script or command output."""
    check, assessment = checks[index], assessments[index]
    ref = check.get("tool_call_ref") or "no tool_call_ref"
    gap = check.get("evidence_gap")
    if check.get("claimed_outcome") == "passed" and gap == "later_run_failed":
        identity = assessment.get("identity")
        later = [run for run in command_history(db, task_id, attempt_id)
                 if identity is not None and _same_command(run["identity"], identity) and run["order"] > assessment.get("order", 0)]
        failing = next((run["ref"] for run in reversed(later) if run["status"] == "failed"), "a later run")
        return (f"passed claim {ref}: a later run of the same command ({failing}) exited nonzero, and the latest "
                f"run decides" + (f"; command {_identity_label(identity)}" if identity else ""),
                "Fix the cause, rerun exactly the same argv and cite that passing receipt; keep the failed run as a "
                "failed check. " + _FAIL_ASSIGNMENT)
    if assessment["strength"] != "verified":
        sentence = EVIDENCE_GAPS.get(assessment["gap"], assessment["gap"] or "no evidence")
        return (f"failed claim {ref} has no verified failing receipt of this attempt "
                f"({assessment['gap']}: {sentence}), so no rerun can close it",
                "Cite the command_run receipt of the run that actually failed. " + _FAIL_ASSIGNMENT)
    identity = assessment.get("identity")
    prefix = ("passed claim " if check.get("claimed_outcome") == "passed" else "failed check ") + ref
    if check.get("claimed_outcome") == "passed":
        prefix += f" exited {assessment.get('exit_code')}: it is a failed run, not a pass"
    if identity is None:
        return (f"{prefix}; its receipt records no command identity, so no rerun can match it", _FAIL_ASSIGNMENT)
    label = _identity_label(identity)
    reason = trivial_reason(list(identity[1]))
    if reason:
        return (f"{prefix}; command {label}. Even an exit-0 run of exactly this argv cannot verify a pass "
                f"({reason}: {EVIDENCE_GAPS[reason]}), so no exact rerun can close this failure",
                'Send assignment_outcome="failed" with this check retained; a different argv never closes it. '
                "For later checks: " + _DIRECT)
    history = command_history(db, task_id, attempt_id)
    later = [run for run in history if run["order"] > assessment["order"]]
    unrecorded = [run["ref"] for run in later if run["identity"] is None]
    same = [run for run in later if _same_command(run["identity"], identity)]
    other = next((other for other, assessed in zip(checks, assessments)
                  if other.get("outcome") == "passed" and assessed.get("identity") is not None and not _same_command(assessed["identity"], identity)
                  and other.get("name") == check.get("name")), None)
    hint = (f" The passed check {other.get('tool_call_ref')} with the same name runs a different command; a "
            f"different argv never closes this failure.") if other else ""
    if unrecorded:
        return (f"{prefix}; command {label}. A later command run of this attempt ({unrecorded[0]}) has no "
                f"recorded argv, so closure is ambiguous and cannot be established", _FAIL_ASSIGNMENT + hint)
    if not same:
        return (f"{prefix}; command {label}. No later run of exactly this command exists in this attempt",
                _EXACT_RERUN + " " + _FAIL_ASSIGNMENT + hint)
    latest = same[-1]
    if latest["status"] == "failed":
        return (f"{prefix}; command {label}. The latest run of exactly this command ({latest['ref']}) failed too",
                "Fix the cause, then " + _EXACT_RERUN[0].lower() + _EXACT_RERUN[1:] + " " + _FAIL_ASSIGNMENT + hint)
    if latest["status"] != "passed":
        latest_gap = _gateway_assessment(db, task_id, attempt_id, latest["ref"], "passed")["gap"]
        return (f"{prefix}; command {label}. The latest run of exactly this command ({latest['ref']}) has no "
                f"established result ({latest_gap}: {EVIDENCE_GAPS.get(latest_gap, 'unknown')})",
                "Wait for or repeat that run until it completes, then cite its passed receipt. " + _FAIL_ASSIGNMENT + hint)
    citing = [i for i, other in enumerate(checks)
              if other.get("outcome") == "passed" and other.get("tool_call_ref") == latest["ref"]]
    if not citing:
        return (f"{prefix}; command {label}. Its latest run {latest['ref']} passed, but no passed check of this "
                f"result cites it",
                f"Keep this failed check and add a passed check citing {latest['ref']}. " + _FAIL_ASSIGNMENT + hint)
    cited_gap = assessments[citing[0]]["gap"]
    detail = (f"is not verified ({cited_gap}: {EVIDENCE_GAPS.get(cited_gap, 'unknown')})" if cited_gap
              else "does not establish a closure")
    return (f"{prefix}; command {label}. The passed check citing the latest run {latest['ref']} {detail}",
            "Cite that receipt from exactly one passed check. " + _FAIL_ASSIGNMENT + hint)


def annotate_checks(db, task_id: str, attempt_id: str | None, checks) -> list[dict]:
    """Read-time copies of stored checks with ``evidence_strength`` (and
    ``evidence_gap`` when weak), derived now under the current rule. Stored
    rows and reports are never rewritten: an older result accepted under a
    weaker rule shows ``evidence_strength='weak'`` beside its outcome."""
    checks = [check for check in checks or () if isinstance(check, dict)]
    if attempt_id is None:
        assessments = [dict(strength="weak" if c.get("outcome") in {"passed", "failed"} else "none",
                            gap="tool_call_ref_unknown" if c.get("outcome") in {"passed", "failed"} else None)
                       for c in checks]
    else:
        assessments = assess_checks(db, task_id, attempt_id, checks)
    currents = (current_outcomes(db, task_id, attempt_id, checks)
                if attempt_id and any(check.get("invocation_error") for check in checks)
                else [None] * len(checks))
    annotated = []
    for check, assessment, current in zip(checks, assessments, currents):
        entry = dict(check, evidence_strength=assessment["strength"])
        if check.get("invocation_error"):
            entry["current_outcome"] = current["outcome"] if current else "unverified"
        for key in ('evidence_source', 'runtime_command_ref'):
            entry.pop(key, None)
            if assessment.get(key):
                entry[key] = assessment[key]
        gap = check.get("evidence_gap") or assessment["gap"]
        if gap:
            entry["evidence_gap"] = gap
        annotated.append(entry)
    return annotated


def resolve_check_tx(db, task_id: str, attempt_id: str, tool_call_ref: str | None, outcome: str = "passed") -> str:
    """The audit label stored on a ``worker_result`` event's check:
    ``observed`` when :func:`assess_check` verifies it, ``contradicted`` when
    the cited command's exit code disagrees with the outcome, else
    ``claimed``. Runs inside an already-open transaction."""
    assessment = assess_check(db, task_id, attempt_id, outcome, tool_call_ref)
    if assessment["strength"] == "verified":
        return "observed"
    if assessment["gap"] in {"nonzero_exit", "zero_exit"}:
        return "contradicted"
    return "claimed"


def resolve_check(store, task_id: str, attempt_id: str, tool_call_ref: str | None, outcome: str = "passed") -> str:
    """:func:`resolve_check_tx` on its own connection."""
    with store.connection() as db:
        return resolve_check_tx(db, task_id, attempt_id, tool_call_ref, outcome)


def record_permission_response(
    store, task_id: str, attempt_id: str, tool_call_ref: str,
    command: str, host_outcome: str,
) -> None:
    """Persist an actual ACP host response for one exact bound action.

    The caller must have checked the engine binding after the host returned.
    A repeated action identity with different command/outcome is a conflict,
    never a fresh approval. No model-authored report can create this row.
    """
    if not tool_call_ref or not command or host_outcome not in {"allow_once", "reject_once", "cancelled"}:
        raise ValueError("incomplete permission response")
    digest = hashlib.sha256(command.encode("utf-8")).hexdigest()
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            row = db.execute(
                "SELECT command_sha256,host_outcome FROM permission_receipts "
                "WHERE task_id=? AND attempt_id=? AND tool_call_ref=?",
                (task_id, attempt_id, tool_call_ref),
            ).fetchone()
            if row is not None:
                if row["command_sha256"] != digest or row["host_outcome"] != host_outcome:
                    raise ValueError("permission response identity conflict")
            else:
                db.execute(
                    "INSERT INTO permission_receipts "
                    "(task_id,attempt_id,tool_call_ref,command_sha256,command_text,host_outcome,created_at) "
                    "VALUES (?,?,?,?,?,?,?)",
                    (task_id, attempt_id, tool_call_ref, digest, command, host_outcome, _now()),
                )
            db.commit()
        except BaseException:
            db.rollback()
            raise
