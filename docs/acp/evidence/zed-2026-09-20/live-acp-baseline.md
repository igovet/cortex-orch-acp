# Isolated Zed live ACP baseline — 2026-09-20

Source identity: `1.0.0+sha256.5d2fa251d908274f`. Client: Zed
`1.20.2+stable.360.7c451e694f3c52ee0aeb01d7e28b5fa18cd0ad2f` on an
isolated Xvfb display and disposable Zed user-data directory. Cortex used
disposable XDG config/state and referenced the existing production native
Codex and Claude accounts without copying credentials. Engines observed:
`codex-acp 1.12.0` and `claude-agent-acp 0.78.0`. All work happened in three
disposable projects outside this checkout; the ordinary Zed process was not
touched. The user explicitly requested a real run and then testing in this
thread. The standalone paid-probe scripts were not invoked.

The results below were read from each project's `.cortex/cortex.sqlite3`,
published Markdown files, exact project files, an independent `unittest`
invocation, and the isolated Zed UI. Private screenshots and raw host logs
remain outside the repository.

| Scenario | Observed route and result | Outcome |
| --- | --- | --- |
| Counter repair | Coordinator Codex Luna; backend-dev Codex Luna. Task `t_a029f958dc42`, attempt `xa_b8103af3390b`, report `r_f853bcb81449`. Worker changed the actual open project's `counter.py` from `sum(values) + 1` to `sum(values)`. Independent `python3 -B -m unittest -v test_counter.py` passed. A separate `decide(assignment_ref="task", decision="accept")` made the durable task state `accepted`. | **PASS** for direct-project execution and autonomous completion. |
| Real-history replay | The same task's host session `acps_ef6f4672742a` retained 23 structured ACP updates (six `tool_call`, 17 `tool_call_update`). After terminating and restarting only the disposable Zed instance, the same chat reopened with the worker card and tool cards visible in order along with the text. | **PASS** for real worker/tool-card replay after restart. |
| Arithmetic repair | Coordinator **Codex** and backend-dev **Claude Sonnet** according to `attempt_effective_context`; task `t_94e990f63b58`, attempt `xa_9e7cc63bd2c1`, report `r_ff04d262b379`. The worker changed the actual open project's `arithmetic.py` from `+=` to `*=`. Independent `python3 -B -m unittest -v test_arithmetic.py` passed and a task-level accept made the state `accepted`. The Zed selector displayed Claude Sonnet before the prompt, so its visible route disagreed with the coordinator's actual Codex route. | **PASS** for this mixed execution; **FAIL** for selector/effective-route consistency. Claude as coordinator remains unverified. |
| Formatting repair | Coordinator Codex Luna; backend-dev Claude Sonnet. Task `t_a9d30678de10`, attempt `xa_15381e1a3f53`, report `r_5b5588f90a10`. Claude changed the actual open project's `formatting.py` from `upper()` to `lower()` and the independent `python3 -B -m unittest -v test_formatting.py` passed. The coordinator then recorded four redundant assignment/attempt `accept` decisions, never recorded a task-level decision, and kept the task running. The turn was stopped through the isolated Zed UI; durable state became `cancelled`. | **FAIL** for bounded autonomous completion, despite a successful worker result. |

Two additional findings apply to the accepted tasks. Their single
`pipeline.md` edition is still the initial runtime draft saying “not yet
worked,” so the user-facing pipeline did not reflect the accepted decision.
The terminal decision event's `consumed_by` and `delivered_at` fields are
null by design: those fields mean delivery to a **later** coordinator
continuation and are not a receipt for a decision made in the current turn.
The implementation reports are immutable and their stored digests match the
files, but the arithmetic report uses a textual post-fix description rather
than a deterministic file digest for artifact provenance.

This baseline does not qualify the source revision after fixes. The reverse
mixed route (Claude coordinator, Codex worker), failure recovery, cancellation
cleanup, and all other unobserved live rows remain unverified.
