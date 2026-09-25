# Verification coverage

Plan verification for every surface the task changes before submitting the plan,
and make it real, not only unit tests:

- Plugin or UI changes: a worker task that observes the change in the isolated
  dev Paseo GUI as AGENTS.md describes, with the plugin path given explicitly.
- Install or packaging changes: a real install from the documented source into
  a disposable environment.
- Runtime, protocol or engine changes: the documented tests plus applicable
  live checks. The owner's standing approval authorizes paid calls for live
  verification by default; run them in isolated dev Paseo under
  `~/.paseo-dev`, with its separate Electron profile, disposable project, and
  Cortex runtime loaded by that dev daemon from the explicit absolute checkout
  plugin path. Use `gpt-5.6-luna` at medium/high effort; heavy models are
  prohibited in routine live qualification. Keep the default automated pytest
  suite no-paid.
- Documentation: commands and links checked against the source.

Name each such check in the worker objective so the worker records it. Do not
ask again for paid-call authorization covered by the standing approval. It
does not authorize use of production Paseo or the production Cortex agent.
Preserve the existing production-account isolation rules and never copy
credentials or create test engine accounts. Paid probe invocations still need
both `--i-authorize-paid-call` and `--approval-ref`; use the standing
reference `owner-standing-2026-09-24`.

While reconciling, use replan_request to close every verification gap that can
be resolved in the authorized environment. List a check as not verified only
when a concrete blocker prevents it; name that blocker in the structured result
and synthesis. Bind each observation to exact, complete, non-replayed evidence; never infer a pass from missing evidence.
When all checks are verified, the final message has no not-verified list.
