# Router and native draft editing: retained live observations

Date: 2026-09-21 UTC. These observations come from the installed Paseo
service and its connected web client, not a substitute client or a local
unit suite. No runbook qualification row is changed by this note.

Task `t_db3053ad759a`, coordinator attempt `co_6e6e30c90787`, used
Codex `gpt-5.6-luna` with high effort. It delegated current-file research to
worker attempt `xa_3659131020ec` (`gpt-5.6-luna`, medium), read the resulting
report `r_b30d57d95f25`, created its own pipeline draft at 15:46:11 UTC,
completed native `Editing files` at 15:46:53, read the draft at 15:47:05,
and completed publication at 15:47:12. It did not delegate that draft edit.
This confirms the native editing/publication sequence for that exact turn;
it does not prove every native edit asks ACP permission or enforce an OS
filesystem boundary.

The web client opened the worker from its coordinator card, displayed the
assignment title, and showed its messages and source-read/command events.
After page reload the worker list returned with running/completed sections;
reopening that worker restored earlier events and subsequent activity.
This is page-reload evidence, not a full desktop or daemon restart test.

Task acceptance was refused after the worker reported unverified server
startup. A successor `xa_168c133e0d2a` retained the same socket-creation
failure in `r_e76b8c2267f2`; successful `--help` was not a successful server.
Both workers still used the former network-disabled policy. The installed
worker-network correction requires a new engine session and a fresh live
observation. This record does not claim completed end-to-end orchestration.

After runtime reload, successor worker `xa_da45b817ebed` published
`r_4bee469199e3`. Its retained native tool output contains the HTTP request
with status 200, and the server process was interrupted after retrieval.
This supplies live worker-network/startup evidence for the revised policy;
the model report alone is not the retained observation. The coordinator then
encountered a completion defect: the gate did not follow a successful result
through more than one revision and did not retire a revised published result
unless it was separately cancelled. That defect was corrected without
rewriting historical results. Final task acceptance after the correction
remains to be observed.
