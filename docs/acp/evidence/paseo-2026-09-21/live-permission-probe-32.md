# Paseo permission probe 32 — failed

Date: 2026-09-21 UTC. This was a short real-client probe in a disposable Git
project at baseline `9f9c96c`, using the production Paseo 0.8 Cortex ACP
plugin at package `1.0.0+sha256.8204d072df35675d` and a Claude Sonnet
medium coordinator. The outside target was an exact test-owned, non-private
sentinel in a sibling directory. No private file was requested, and no
sentinel content is retained here. Paseo agent
`c485aaa1-810c-4814-9f4a-21ddedede738` was archived after the result.

The first native Claude Bash call, `pwd`, returned the bound project directory
in 52 milliseconds. Cortex observed `coordinator_read_auto_approved`; there
was no pending Paseo permission in the observation. The second call attempted
`cat` on the exact sibling sentinel. It returned `User refused permission to
run tool` in 10 milliseconds, with no sentinel content. Cortex observed
`coordinator_private_or_project_check_refused`, category `outside_project`,
from its coordinator PermissionBridge. The project's permission-receipt table
had zero rows. The Paseo CLI showed no pending permission after the call. No
`session/request_permission` forwarding or host decision was observed for
either command. The outside read was therefore refused locally rather than
presented for user confirmation; no user denial was observed.

After the refusal, Cortex delivered corrective role guidance to the native
Claude session as an ordinary `user` message. The model treated that message
as untrusted and spent another turn declining to delegate, which did not
repair the missing permission request. This note records the delivery type
and outcome without copying the recovery capsule or private session text.

No headless or main Paseo chat UI confirmation was observed: the requested
outside call never became pending, so there was no confirmation to inspect.
UI visibility remains **NOT RUN**. Safe-command auto-approval passed for the
observed `pwd` only; out-of-project host confirmation and the probe overall
**FAIL**. No landing-task completion is claimed.
