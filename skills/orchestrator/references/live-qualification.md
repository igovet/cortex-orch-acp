# Live verification

Use only capabilities verified in the current installed environment. Bind each observation to its exact task, session and artifact. Missing, ambiguous, truncated or replayed evidence cannot establish a passing result. Unit tests and fake sessions support only the behavior they exercise.

There is no standing approval for paid model calls. Run a paid live check only
when the owner explicitly requests it and names the mode: `cli` (headless,
through the guarded Paseo CLI, checked against the disposable project's
`.cortex` store) or `desktop` (the Paseo Electron app, for rendered behavior).
Otherwise report the check as not run with the blocker "paid live check not
requested by the owner". Run requested checks through isolated dev Paseo: use the
permanent `~/.paseo-dev` daemon, its separate Electron profile at
`~/.config/Paseo-dev`, a disposable project, and the Cortex runtime started by
that dev daemon from the explicit absolute checkout plugin path. Never use
production `~/.paseo` or its production Cortex agent. Existing production
Codex and Claude accounts remain the approved engine accounts; do not create
test accounts, copy credentials, or perform isolated engine login. Check native
login status without printing credentials. Use only the exact route explicitly selected by the owner for this requested check, and confirm it is supported by the installed host and current owner settings/catalog. Never substitute or automatically escalate to another route. Heavy models remain prohibited in routine live qualification.

The paid probes still require both `--i-authorize-paid-call` and
`--approval-ref`; pass a reference naming the owner's request. The default automated suite stays no-paid.
