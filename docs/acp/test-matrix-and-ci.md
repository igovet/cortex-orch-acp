# Automated test matrix, CI commands and the live-test queue

Written at T034. This page states how the mandatory automated matrix is run,
what guarantees a default run cannot spend money, and which acceptance cases
automated evidence deliberately cannot close.

Current candidate coverage includes deterministic DAG scheduling, authenticated
file/command/external MCP gateways, mandatory review, exact revision verification,
steering, bounded recovery and unfiltered transcript replay. Authorized real Codex Luna and Claude Haiku tasks ran through isolated Paseo;
see the [retained observations](evidence/paseo-2026-09-21/runtime-rewrite.md).
The standalone paid probe scripts were not used in this campaign. See [current results](runtime-rewrite-status.md).
No passing offline test changes a NOT RUN real-client row into a pass.
The broker suite forces a random token whose first byte is `-`, then checks
that the actual stdio relay still connects and advertises tools. It also
checks an MCP child that exits before `tools/list` and the prompt-level error
classification. Those no-paid checks cannot establish a real client's MCP
availability; its actual first prompt remains a live observation.

## The non-paid default command

```
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q
```

Two environment requirements are not cosmetic; both were learned by losing
runs to them:

- `PYTHONDONTWRITEBYTECODE=1` remains required for candidate checks and is
  recommended for the suite. Subprocesses do not inherit `python3 -B`, only
  the environment variable. Package enumeration also excludes local
  `__pycache__`, `.pyc`, and `.pyo` residue from the shipped payload and
  isolated candidate copy; these execution artifacts are never package
  inputs, so stamping does not require destructive cleanup of ignored files.
- `HOME` and `TMPDIR` must lie outside **every** git repository, ancestors
  included. The storage guards refuse to treat a directory inside a checkout
  as a user home, and a scratch home under a user directory that is itself a
  dotfiles repository produces false failures far from their cause.

For a release-grade run, verify in a throwaway clone rather than the working
checkout: `git clone --no-hardlinks`, overlay the working diff, stamp there
(`python3 -B scripts/cortex_package.py stamp`), then run the command above.
The stamp must reproduce byte-identically in the real checkout.

## Why a default run cannot start a paid campaign

Nothing in the default suite can reach a paid engine, and this is enforced in
the product, not by convention:

- The paid probe (`scripts/cortex_codex_probe.py`, and its Claude
  counterpart) refuses to do anything unless **both** `--i-authorize-paid-call`
  and `--approval-ref` are supplied. Either flag alone exits non-zero before
  any engine is loaded. The probe test suite never passes them.
- Tests that spawn the real `cortex_acp.py` entry point run against a fresh
  temporary `HOME`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME` with no connections
  configured, so `session/prompt` fails closed at route resolution before any
  engine process could be launched — regardless of which engine binaries
  happen to be on `PATH`.
- Diagnostics are covered by a test that instruments the real engine-launch
  constructor and asserts zero launches during a full diagnostic run.

A live campaign therefore requires an explicit, separately authorized
invocation. It cannot be reached by running the test suite.

## CI note: run as a non-root user

One test (`tests/test_artifact_revision.py`, unreadable-entry handling) skips
itself when the process can bypass directory permissions. A containerized CI
runner executing as root would silently never run it. Configure CI with a
non-root user. The suite otherwise has no skips: as of T034 it is
**2053 passed, 0 failed, 0 skipped**.

Remaining conditional guards are platform guards (POSIX FIFO support,
symlink support, `tmux` presence) whose logic is also covered by unconditional
tests; they are not the sole coverage of any mandatory case.

## What automated evidence cannot close

Of the 69 mandatory acceptance cases, **45 require live evidence** —
observations from real clients and real engines that no fixture may stand in
for. Automated coverage for these rows is contract evidence only and is never
to be recorded as satisfying them.

| Group | Cases | Owning tasks |
| --- | --- | --- |
| Client experience (UI) | UI-01 … UI-11 | T035 |
| Engine execution (EX) | EX-01 … EX-07, EX-10 | T035, T036 |
| Recovery and context (RC) | RC-01 … RC-12 | T037 |
| Security (SEC) | SEC-01 … SEC-05 | T035, T036 |
| Protocol dialogue | PR-08 | T036 |
| Deployment | DEP-01 | T035 |
| Qualification | QUAL-02 … QUAL-08 | T037, T038 |

A fixture-driven fault injection stays fixture evidence even when it runs
inside a live campaign. An unrun live case is never labelled passed.
