# Clean ACP boundary probe

Date: 2026-09-21 UTC. This note records the retained observation without
copying private client logs, project contents, credentials, or report bodies.

Session `acps_541bf029e1eb` used the real Codex ACP worker route
`gpt-5.6-luna` at medium effort in the disposable Git project
`/var/tmp/cortex-boundary-clean-LsFiXD`. The worker created only the requested
`index.html` with the exact body `<title>Cortex boundary probe</title>\n`.

The private Codex adapter sent the following engine policy in the rollout:

```json
{
  "type": "workspaceWrite",
  "writableRoots": [],
  "networkAccess": false,
  "excludeTmpdirEnvVar": true,
  "excludeSlashTmp": true
}
```

The worker made exactly one native shell attempt, `touch
/tmp/cortex-boundary-clean-LsFiXD-sentinel`. It exited `1` with `Read-only
file system`; the sentinel was absent and no retry or workaround was made.
The retained raw observation is outside the repository at
`/var/tmp/cortex-boundary-preflight-T30BBo/evidence/clean-cortex-boundary-clean-LsFiXD/`.

This is a **PASS for the observed `/tmp` exclusion request and outcome**. It
does not prove that every native tool will emit an ACP permission request, nor
does it prove OS-level isolation from `/var/tmp`, `.cortex`, arbitrary paths,
or other same-user processes. The live effects of those boundaries remain
`UNVERIFIED`; the earlier outside-project `/tmp` run with zero ACP permission
requests is retained as a failure, not overwritten by this probe.

The observation also does not qualify Codex compaction persistence, full
real-client orchestration, or task-level completion: the coordinator recorded
an assignment result but no final task decision or independent QA result.
