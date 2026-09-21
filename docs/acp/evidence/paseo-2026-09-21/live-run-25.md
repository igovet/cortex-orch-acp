# Twenty-fifth Paseo live qualification — failed

Date: 2026-09-21 UTC. This note records the retained qualification result
without copying private client logs, project contents or credentials.

The run used the disposable Git project
`/var/tmp/cortex-paseo-landing-twenty-fifth-20260921-ElU9lp/project`, the
`1.0.0+sha256.d2ab42739d998e71` package, Paseo agent
`bbe39f09-3c79-47c4-b895-239fbaa4ba13`, and Claude Sonnet medium as the
coordinator. The initial good and forced-bad browser controls behaved as
expected before launch.

The Codex implementation worker published report `r_c6e5dd36080a`. Its static
checks passed, while its native rendered-focus check was `UNVERIFIED` because
the browser checker exited 2 on the host's crashpad `setsockopt: Operation not
permitted` error. The native rollout retained the parallel pytest and Git
checks, but the ACP worker history omitted the parallel rendered-check call
and its stderr. This is a worker-history visibility defect; the exit 2 is not
a pass.

An independent Claude Sonnet QA worker read the exact predecessor report and
published `r_2a0a96610c1d`. Its static checks and rendered checker passed,
including `focusVisible:true`, a solid white 3px outline with 4px offset, and
`PASS`/`EXIT:0`. The publication's structured `worker_result.checks` array
was empty, so its prose and native command output could not satisfy the
structured acceptance gate.

The coordinator created candidate `cand_a114036cd781d38527bcb9ac3e922bc8`
and bound checks directly observed both the rendered checker and pytest as
passing. Acceptance was still refused because the implementation worker's
rendered check remained unverified, the QA publication had no structured
check, and one candidate assignment had no `worker_result`. Repeated unchanged
acceptance attempts did not resolve those defects. The coordinator later
authored a newer pipeline edition, but no valid task decision followed.

The disposable Paseo agent was stopped and archived. The Cortex task ended
`stopped` with zero durable task-decision events; no acceptance is claimed and
no product or plugin source was edited during this run.

Remediation remains bounded to three items: preserve exact structured QA
checks for decisive observations, allow the gate to consume a bound current
candidate check only through an explicit evidence path, and project every
nested command from parallel native `functions.exec` calls into ACP history.
The coordinator must also recover by requesting a corrected report or new
decision rather than retrying an unchanged refused acceptance.
