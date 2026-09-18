# Real CLI and Desktop qualification

Load only for explicit real-host qualification. Follow the target repository's
verified launchers, observation commands and current-host capabilities. The workload
is ordinary product work; do not embed orchestration acceptance instructions in it.

The Codex-plugin-harness-era isolated cortex-dev candidate and CLI-before-Desktop
launchers (`scripts/cortex-dev`, `scripts/cortex-desktop-dev`,
`scripts/cortex-live-smoke`) are historical, superseded by the T033 ACP migration;
`scripts/cortex-dev` was deleted as genuinely dead, and the surviving two files are
kept only as a coordinator/worker policy reference, not a runnable launcher (see
AGENTS.md's "Historical: Codex-plugin/native-subagent CLI-before-Desktop
qualification procedure"). Real qualification against a live ACP client follows
that client's own verified launcher and observation commands instead: record exact
candidate, launcher and observer hashes and verify the workdir has the required
files. Confirm trust and composer visibly, submit once, retain complete calls/events
and observe natural completion. Pending is not failure.

Only after CLI acceptance with complete outcome and clean applicable audit, run real
Desktop on the unchanged payload with a disposable profile. Unsupported attestations
remain diagnostics under the documented observational boundary; actual task, worker,
report and artifact proof is still needed.

For a failed attempt preserve all current calls/events/audit before exact owned-session
cleanup. Never kill a tmux server or overwrite sealed pre-stop evidence with no-session
observations. Record the new causal evidence before retrying. State each host's actual
outcome; offline checks or partial worker results do not prove live parity.
