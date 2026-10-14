"""Published deny-write examples stay truthful.

Every backticked ``deny-write: <value>`` directive in the current product
references must be a real, usable denial: it parses to exactly one path, the
gateway's deny-write matcher refuses that path (and a descendant when the
example names a directory), and no published example relies on a wildcard,
which the parser never accepts as a new denial.

Angle-bracket placeholders such as ``deny-write: <path>`` describe the syntax;
they are not published examples and are excluded explicitly.
"""
import re
from pathlib import Path, PurePosixPath

from cortex_runtime.orchestration.gateway import write_denied
from cortex_runtime.orchestration.policy import parse_denials

ROOT = Path(__file__).resolve().parent.parent
DOCS = (
    ROOT / 'SECURITY.md',
    ROOT / 'docs' / 'features' / 'permissions-and-containment' / 'index.md',
)
# A whole backticked directive; the value stops at the closing backtick.
DIRECTIVE = re.compile(r'`(deny-write\s*:\s*[^`\n]+?)\s*`')
PLACEHOLDER = re.compile(r'^deny-write\s*:\s*<[^<>]+>$')
WILDCARD = re.compile(r'[*?\[]')


def _published_directives():
    found = []
    for doc in DOCS:
        text = doc.read_text(encoding='utf-8')
        for match in DIRECTIVE.finditer(text):
            directive = match.group(1)
            if PLACEHOLDER.match(directive):
                continue
            found.append((doc.relative_to(ROOT).as_posix(), directive))
    return found


def test_published_docs_contain_deny_write_examples():
    assert _published_directives(), 'no backticked deny-write example found'


def test_published_deny_write_examples_contain_no_wildcards():
    offenders = [(doc, text) for doc, text in _published_directives() if WILDCARD.search(text)]
    assert offenders == []


def test_each_published_deny_write_example_parses_to_one_denied_path():
    for doc, directive in _published_directives():
        parsed = parse_denials(directive)
        assert len(parsed) == 1, (doc, directive, parsed)
        path = parsed[0]
        assert write_denied(path, [path]), (doc, directive, path)
        # Negative control: an unrelated project path is not covered.
        assert not write_denied('unrelated/other.txt', [path]), (doc, directive, path)
        if not PurePosixPath(path).suffix:
            # A directory example also refuses a descendant path.
            assert write_denied(f'{path}/descendant.txt', [path]), (doc, directive, path)


def test_wildcard_deny_write_directive_creates_no_denial():
    assert parse_denials('deny-write: .claude/settings*.json') == []
