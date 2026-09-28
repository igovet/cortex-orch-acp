# Automated test matrix and CI commands

This page documents the no-paid automated suite and its limits. Paid live checks run only on an explicit owner request naming `cli` or `desktop`, through the isolated dev Paseo environment documented separately.

Current candidate coverage includes deterministic DAG scheduling, authenticated
file/command/external MCP gateways, mandatory review, exact revision verification,
steering, bounded recovery and unfiltered transcript replay. Authorized real Codex Luna and Claude Haiku tasks ran through isolated Paseo;
see the [retained observations](evidence/paseo-2026-09-21/runtime-rewrite.md).
The standalone paid probe scripts were not used in this campaign. See [current results](runtime-rewrite-status.md).
The broker suite forces a random token whose first byte is `-`, then checks that the stdio relay still connects and advertises tools. It also checks an MCP child that exits before `tools/list` and prompt-level error classification. These tests cover the runtime broker behavior exercised by the fixtures.

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

### Parallel run

With [pytest-xdist](https://pypi.org/project/pytest-xdist/) installed
(`pip install --user pytest-xdist`), the same suite runs across all cores:

```
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q -n auto --dist loadgroup
```

Use `--dist loadgroup`. It spreads tests one by one and keeps tests marked
`xdist_group` on one worker. `tests/test_emission_scale.py` uses those marks
so each module-scoped scale run happens once per session, not once per
worker. On a 20-thread workstation the serial run takes about 6 minutes and
the parallel run about 65 seconds. The two commands select the same tests.
Without pytest-xdist, the serial command still works and the `xdist_group`
marker has no effect.

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
non-root user. Current totals depend on the checkout and environment; record
the exact passed, skipped and failed counts from each run rather than relying
on the historical T034 count.

Remaining conditional guards are platform guards (POSIX FIFO support,
symlink support, `tmux` presence) whose logic is also covered by unconditional
tests; they are not the sole coverage of any mandatory case.
