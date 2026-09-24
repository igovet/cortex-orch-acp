# Real-client qualification

Use only capabilities verified in the installed client and engine. Keep every
observation bound to one exact task, session and artifact. Missing, ambiguous,
truncated or replayed evidence cannot establish a passing result. Follow the
project's real-client runbook in disposable environments; retain NOT RUN for
unobserved rows. Unit tests and fake engine sessions are not live qualification.

The owner's standing approval authorizes paid model calls by default for
applicable live checks. Run those checks through isolated dev Paseo: use the
permanent `~/.paseo-dev` daemon, its separate Electron profile at
`~/.config/Paseo-dev`, a disposable project, and the Cortex runtime started by
that dev daemon from the explicit absolute checkout plugin path. Never use
production `~/.paseo` or its production Cortex agent. Existing production
Codex and Claude accounts remain the approved engine accounts; do not create
test accounts, copy credentials, or perform isolated engine login. Check native
login status without printing credentials. Use `gpt-5.6-luna` at medium/high
effort; heavy models are prohibited in routine live qualification.

The paid probes still require both `--i-authorize-paid-call` and
`--approval-ref`. The standing approval reference is
`owner-standing-2026-09-24`. This authorization does not change the no-paid
default automated suite or permit marking an unobserved runbook row passed.
