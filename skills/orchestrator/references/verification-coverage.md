# Verification coverage

Plan verification for every surface the task changes before submitting the plan,
and make it real, not only unit tests:

- Plugin or UI changes: the plugin type-check and the
  `npm --prefix integrations/paseo run test:*` scripts covering the change.
  Add a worker task observing the change in the isolated dev Paseo GUI only
  when the owner's request asks for a `desktop` check.
- Install or packaging changes: a real install from the documented source into
  a disposable environment.
- Runtime, protocol or engine changes: the tests covering the changed code and
  the no-paid smoke harness where it applies. Plan a paid live check only when
  the owner's request explicitly asks for one and names the mode, `cli` or
  `desktop`; run it in isolated dev Paseo under `~/.paseo-dev`, with a
  disposable project and the Cortex runtime loaded by that dev daemon from the
  explicit absolute checkout plugin path. Use only the exact route explicitly selected by the owner for this requested check, and verify it is supported by current settings and the installed host/catalog; do not automatically substitute or escalate to the deep coordinator. Heavy models remain prohibited in routine live qualification. Keep the
  default automated pytest suite no-paid.
- Documentation: commands and links checked against the source.

Name each such check in the worker objective so the worker records it. Worker
checks are targeted: name the test files or selection that covers the change,
never the project's whole suite. Put the whole documented suite only in
`plan.verification`; the runtime runs it once after review, with no time limit. There
is no standing paid-call approval: without an explicit owner request naming
`cli` or `desktop`, launch no paid model call and no live Paseo run, and list
each applicable live check as not verified with the blocker "paid live check
not requested by the owner". A request never authorizes production Paseo or
the production Cortex agent. Preserve the existing production-account
isolation rules and never copy credentials or create test engine accounts.
Paid probe invocations still need both `--i-authorize-paid-call` and
`--approval-ref`, with a reference naming the owner's request.

While reconciling, use replan_request to close every verification gap that can
be resolved in the authorized environment. List a check as not verified only
when a concrete blocker prevents it; name that blocker in the structured result
and synthesis. Bind each observation to exact, complete, non-replayed evidence; never infer a pass from missing evidence.
When all checks are verified, the final message has no not-verified list.
