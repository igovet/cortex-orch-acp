# Twenty-eighth Paseo live qualification — failed

Date: 2026-09-21 UTC. This note records the retained qualification result
without copying private client logs, project contents, credentials, or report
bodies.

The run used a disposable Git project and a real Paseo ACP session. Claude
Sonnet medium coordinated the task; Codex Luna implemented it; Claude Sonnet
performed the independent QA review. The implementation and QA workers both
published reports, and the independent project checker passed all eight static
checks. The task was stopped before a complete coordinator/decision chain was
recorded.

## Confirmed MCP contract failure

The coordinator first called `read_report` with the worker result's
`report_ref` field:

```json
{"report_ref":"r_507425ce780f"}
```

The advertised `read_report` contract accepts `report_id`, not `report_ref`,
so Cortex returned `invalid_arguments` and identified `report_ref` as an
unadvertised field. The retry used the correct contract:

```json
{"report_id":"r_507425ce780f"}
```

and succeeded. This is a real Cortex MCP error and violates the zero-MCP-error
qualification criterion. The report reference in worker handoff metadata and
the report identifier required by `read_report` were not normalized at the
coordinator boundary.

## Observed result and limits

- The implementation worker published report `r_507425ce780f`.
- The QA worker published report `r_bc43a8eb6baa`.
- The independent project checker exited 0 and passed all 8 static checks:
  required files, required content, semantic landmarks, headings, image alt
  text, responsive CSS, focus style, and no remote dependencies.
- QA recorded the rendered-browser/accessibility check as `not_run`; no
  rendered pass was inferred.
- The reports' structured checks were retained as claims and were not promoted
  to observed rendered evidence.
- No overall task-level decision was recorded before the run was stopped.
- Same-session missing-publication recovery was not exercised because the
  implementation worker published on its first turn.
- The provider-catalog `project_storage_mismatch` observation occurred before
  launch, but the task itself did launch; it is separate from the confirmed
  MCP contract failure.

This run is **FAIL**, not a transport-crash diagnostic. The retry succeeding
does not erase the first MCP error, and the static checker does not establish
the unrun rendered requirement or task-level completion. The retained private
source observation is outside the repository at
`/var/tmp/cortex-paseo-landing-twenty-eighth-20260921-100339/evidence/failed-run.md`.
