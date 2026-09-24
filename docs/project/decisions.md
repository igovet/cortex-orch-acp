## 2026-09-23 audit remediation (user-approved)

- D1 build-verification profile: keep it (all 23 profiles are required);
  document it as runtime-reserved — not plannable, not dispatched — since the
  runtime itself executes command verification. Approved in
  `list_sources`/`read_source(pq_d977d180d44f:answer)`, re-confirmed in
  `read_source(pq_d4502996482e:answer)`.
- D2 retention: ship a user-run maintenance CLI, `python3 src/cortex_clear.py
  --project-root <path> --days N [--keep-thread <id>]`; the coordinator only
  explains it. Codex-era `CODEX_*` env protection is removed. Approved in
  `read_source(pq_d977d180d44f:answer)`, re-confirmed in
  `read_source(pq_d4502996482e:answer)`.
- D3 `docs/project/verification.md` is produced by a knowledge-harvest
  refresh rather than hand-written, per the user, in
  `read_source(pq_d977d180d44f:answer)`, re-confirmed in
  `read_source(pq_d4502996482e:answer)`.
- Worker routing: a route-caused failure (quota/rate limit, model
  unavailable, engine start failure, unconfirmed worker mode) is recorded
  unavailable with an expiry and retried on another available route; routing
  refusals (pin/deny/malformed `routing.json`/no available route) are named,
  non-retried blockers. Haiku workers configure only options their own
  session offers. Required by the user in
  `read_source(pq_d9fa85b27e5c:answer)`.
- Live checks for this remediation are user-authorized paid Claude-only runs
  in an isolated dev setup; the production plugin and the user's real
  configuration are never touched. Requested via
  `read_source(pq_bc3b03c3f491:answer)` (runtime fix to make answers visible
  to reviewers, which this citation form now reflects).

## 2026-09-24 verification gap disclosure

A structured unrun or unverified worker check carries a concrete blocker. At
synthesis, retained gaps from the current plan revision must either have a
same-name passing result or appear in the structured not-verified list with a
blocker. The runtime renders the list into the stored synthesis. This prevents
silent omission while leaving the coordinator able to replan verifiable gaps.

## 2026-09-24 standing live-call approval and isolated Paseo decision

The owner authorized paid model calls by default for live verification and
directed that such testing run through the isolated dev Paseo instance. For
applicable live checks, use the permanent `~/.paseo-dev` daemon, its separate
Electron profile at `~/.config/Paseo-dev`, a disposable project, and the
Cortex runtime launched by that dev daemon from the explicit absolute checkout
plugin path. Never run these checks through production `~/.paseo` or the
production Cortex agent. The existing production Codex and Claude accounts may
be used through the approved isolated setup; do not create test accounts, copy
credentials, or perform isolated engine login. Check native login status
without printing credentials before relying on either engine.

The default live model remains `gpt-5.6-luna` at medium/high effort; heavy
models remain prohibited in routine live qualification. This standing approval
authorizes the paid probe flags with approval reference
`owner-standing-2026-09-24`; the probe scripts still enforce both
`--i-authorize-paid-call` and `--approval-ref`. The default automated pytest
suite remains no-paid. This decision does not change evidence rules: no
unobserved real-client runbook row may be marked passed.
