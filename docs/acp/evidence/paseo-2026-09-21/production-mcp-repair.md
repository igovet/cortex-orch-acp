# Production Paseo MCP registry repair — 2026-09-21

The prior isolated qualification did not cover the owner's existing MCP registry.
Production ACP session creation failed because enabled `codebase-memory-mcp` and
`playwright` entries lacked the runtime gateway's mandatory exact tool grants.
This was a real update regression, not a passed qualification case.

The owner registry was backed up privately and repaired from actual installed
`initialize`/`tools/list` responses. Codebase-memory 0.10.8 exposes 15 exact tools;
Playwright exposes 25 permitted browser tools, excluding arbitrary server-process
JavaScript (`browser_run_code_unsafe`). The existing worker/Claude-only Playwright
filter and disabled Google Workspace entry were preserved. Codebase-memory gets
only its binary read mount and no network. Its graph cache is private to each
contained MCP process, rather than sharing the native account daemon/index.
Playwright gets explicit Node, package, selected installed Chromium and font
mounts, an executable path and network permission. Private user browser profiles
are not mounted. Public system DNS/hosts/NSS and CA files accompany that network
grant; the complete `/etc` directory is not exposed.

Real installed servers were exercised through the production external gateway,
using authenticated fixture worker channels in a disposable project. Codebase
indexing returned `indexed` with 17 nodes and 16 edges and no uncovered files for
the tiny fixture. Playwright opened `https://example.com/` and returned its title,
`Example Domain`. The initial incorrect `path` argument to codebase indexing was
rejected; the successful call used its advertised `repo_path` field. Actual MCP
responses and the isolated store were retained privately outside the repository.
These are real tool checks, not model tasks or a complete browser workflow audit.

The updater now validates the same owner registry before mutations and explicitly
reloads an existing running plugin. Regression tests cover an old entry failing
before any Paseo calls, preserving the registry, real plugin reload dispatch,
Playwright entry repair without broadening engine filters, and network isolation
with system resolver/CA visibility only under an explicit network grant.

The repaired updater ran successfully against the user's running Paseo 0.8.0
production daemon. A fresh disposable project then opened through its real
`cortex` provider and completed `/cortex-status`; the retained Paseo timeline
contains both the command and the returned configuration status. This built-in
command does not invoke a model. No new paid model tasks were run in this repair.

Validation after the repair: all 57 targeted regression checks passed, followed
by the complete no-paid Python suite: **2,356 passed, no skips** (146.90 seconds),
with installed Codex binary/adapter checks explicitly enabled. Package validation
and generation checks for all 23 profiles passed. TypeScript was unchanged in
this repair; no additional GUI/model campaign is claimed.
