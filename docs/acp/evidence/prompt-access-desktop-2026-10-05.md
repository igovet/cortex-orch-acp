# Prompt/access desktop qualification — 2026-10-05

## Decision

**Partial live qualification, not full Part D closure.** Both owner-selected
coordinators ran in isolated Paseo Desktop and both disposable Cortex tasks
reached runtime `COMPLETED`. Worker recovery capsules and offered Cortex tool
lists were observable; coordinator skill reads, consultants, and the paired
Guarded/Auto outside-write behavior were exercised.

The native-write refusal control was falsified: an ordinary writing worker
could write elsewhere inside its project. The actual refused operation was an
invalid wildcard `access_request`, followed by a new exact-path request and
continuation. This qualifies validation recovery, not a filesystem-denial
boundary. Codex consultant native source reading and Paseo retained-log
completeness remain **unverified**. Redacted recovery capsules are not pristine
system-prompt wire captures.

## Authorization and environment

- Parent task: `t_9f911bfb173e`; worker attempt: `xa_31891400bb31`.
- Owner explicitly requested desktop, both engines and all Part D scenarios.
  Clarification `pq_c1abfe9ffccd:answer` selected Sonnet/Luna coordinators and
  unchanged production routing policy for workers. This replaced the initial
  light-only constraint; automatic workers included Sol and one Opus reviewer.
  No manual route substitution or production configuration write was issued.
- Installed Paseo: **0.10.3**, rechecked; historical versions were not reused.
- Daemon home `~/.paseo-dev`; Electron profile `~/.config/Paseo-dev`;
  endpoint `127.0.0.1:16767`; server `srv_dLC3aKF5GZxO`.
  Wrapper isolation, installed bridge environment support, distinct production
  and dev identities, and loopback DevTools were checked before launch.
- Plugin registration was enabled/running from the absolute checkout path
  `/home/igovet/Web_Projects/cortex-orch-acp/integrations/paseo`.
  Install returned exit 1, “already configured”; the subsequent running exact
  registration was independently observed. No production installation changed.
- Both existing native account logins were observed successful; credentials and
  account identifiers were not printed, copied or placed in disposable homes.
- Dev/production routing policy hashes matched:
  `7067e5ed7913057a398a5a32e610ccab72c56bb6862c4ee57eaf8c30346c2673`.
- GUI: Electron with private Xvfb, **1440×900**, loopback CDP
  `http://127.0.0.1:53881`. Playwright drove visible controls; X11 frame
  captures were visually inspected, not merely inferred from CDP/DOM.
- The checkout itself was never a paid workload target. Raw logs, message
  bodies and screenshots are not embedded in this repository evidence.

## Exact run bindings

| Field | Sonnet / Guarded | Luna / Auto |
| --- | --- | --- |
| Disposable project | `/tmp/cortex-prompts-desktop-xa318-c5ibefg3` | `/tmp/cortex-prompts-luna-xa318-_0sjdpi8` |
| Host agent | `a864d95f-4675-493d-801e-0920bde9d29f` | `7d9fe8b7-81f8-4714-8f2d-81e3ec7bc6ab` |
| Workspace | `wks_40c736548d374443` | `wks_8542de4aa90ae2bb` |
| ACP session | `acps_94fb27ae6728` | `acps_eede988697c4` |
| Task / epoch | `t_1dd384d597de` / 1 | `t_d0a533d99510` / 1 |
| Applied coordinator | `claude-sonnet-5`, medium, default | `gpt-6-luna`, high, agent |
| Final runtime state | COMPLETED, revision 4, review cycle 1 | COMPLETED, revision 2, review cycle 0 |
| Final timestamp UTC | 01:44:22.098528 | 01:33:19.912766 |
| Binding digest | `3c45583ed42bd72e3e23fbd097ba6d2bfc0c279c93c6d5d86ac728eb8d880666` | `30b9cb9368b817fb5b69f9967b714c7e30039c70af1e87a049be8a0a603c85dd` |

The supported dev binding exporter fetched/refetched each exact owned agent and
verified durable ownership. No direct SQLite/private-store workaround was used.
Sonnet's initial busy inspection was refused; it was retried only after the
rendered task changed to completed.

All bounded metadata pages were retained through their exact returned cursors:

| Run | Rows / pages | Stable snapshot digest | Final-page receipt |
| --- | --- | --- | --- |
| Sonnet | 1371 / 14 | `21a41f662202b8fb0149f48841aa9b9956e31a52d49c1bc9ea133c121329c989` | `gateway:8242438475404faea00a13191e04d86e` |
| Luna | 597 / 6 | `3ca268075f50b351f7ce654f3156a54f9a4bd765989a16f0e361b61b47baf248` | `gateway:38dd0150305643c581c5e1f31cca951d` |

Sonnet metadata includes 1062 task events and 166 runtime events; Luna includes
417 and 99 respectively. This complete metadata observation **does not prove**
Paseo's separate log history is complete.

## Criteria-to-scenario matrix

| Criterion | Result | Observed evidence / limit |
| --- | --- | --- |
| Writing-worker recovery capsule and offered Cortex tools, both runs | Pass, scoped | Full displayed redacted `read_capsule` JSON, no omitted/referenced fields; writer list below. Not the unredacted system-prompt payload. |
| Read-only-worker recovery capsule and offered Cortex tools, both runs | Pass, scoped | Explorer capsules and 20-tool lists below. No mutating Cortex project tools offered. Native engine tools are separate. |
| Deliberate rejection followed by access request | Pass for validation only | `checks/**` rejected as `unsafe_path`; question answered, new exact directory `checks` request and continuation. Not a native permission denial. |
| Native write outside declared file scope | **Failed control** | Sonnet worker wrote `checks/refusal-probe.txt` despite scope `src/greeting.py`; capsule explicitly says `already_project_writable`. This matches whole-project writer access, not a new containment guarantee. |
| Coordinator reads `verification-coverage` | Pass | Both rendered timelines show the actual skill tool call. |
| Consultant invoked / granted Cortex set | Pass | Both consultants ran on Codex Sol; 17-tool capsule list below excludes project/command tools. |
| Codex consultant native source-read capability | **Unverified** | Consultant did not attempt the native read because its role forbids source inspection. Instructional refusal is not proof of technical denial or success. |
| Guarded outside action | Pass | Sonnet `outside_write` displayed an Allow/Deny card; tester approved through the visible UI; exact resulting bytes verified. |
| Auto outside action | Pass, paired scenario | Luna `outside_write` succeeded without an outside-write permission-card action by tester; Auto mode visible. An access-question form was separate, not an outside approval. |
| Rendered timeline | Pass | X11 captures inspected: worker catalogue, Guarded permission card, completed Sonnet pipeline and Luna completion timeline. |
| Runtime completion | Pass | Both bound metadata snapshots say COMPLETED; rendered final state corroborates. Host idle alone was not the oracle. |
| Paseo retained-log completeness | **Unverified** | Current no-tail `logs --json` returned non-JSON text with no independently established event identity/completeness signal. |
| Full unredacted prompt wire text | **Unverified** | Recovery JSON is observable but redacts hashes/references and is not the wire system prompt. |
| Cross-engine permission-mode matrix | Not exercised | Sonnet/Guarded and Luna/Auto were paired; Sonnet/Auto and Luna/Guarded were not separate runs. |

### Capsule identities and sizes

Sizes are displayed JSON characters, not original prompt bytes. All six displayed
capsules reported compaction level 0, Verbatim, zero omissions/references.

| Run / role | Attempt | Displayed characters | Actual native worker route |
| --- | --- | ---: | --- |
| Sonnet explorer | `xa_830678637c57` | 14892 | Luna high |
| Sonnet consultant | `xa_21e9f8186c30` | 14501 | Sol medium |
| Sonnet writer | `xa_b69ada098ddf` | 18016 | Sol medium |
| Luna explorer | `xa_c0202a465add` | 12598 | Luna high |
| Luna consultant | `xa_8030933d01d8` | 12624 | Sol high |
| Luna writer | `xa_72891670369b` | 15211 | Sol medium |

Recovery `route_and_config`, labeled `own_provisioning_snapshot`, repeated the
coordinator route even for differently routed workers. Actual native worker
configuration confirmations and rendered worker cards instead show Luna/Sol.
Treat that recovery label as ambiguous; do **not** use it as proof of a worker's
native route. This discrepancy is retained for the coordinator, not fixed here.

Exact offered Cortex tool names, identical per role across the two tasks:

- **Explorer (20):** access_request, ask_question, command_run, create_draft,
  draft_write, escalation_request, list_reports, list_sources, project_git,
  project_read, read_capsule, read_draft, read_large_output, read_report,
  read_skill, read_source, replan_request, result_submit, scope_request,
  write_report.
- **Consultant (17):** access_request, ask_question, create_draft, draft_write,
  escalation_request, list_reports, list_sources, read_capsule, read_draft,
  read_large_output, read_report, read_skill, read_source, replan_request,
  result_submit, scope_request, write_report.
- **Writer (26):** access_request, ask_question, command_run, create_draft,
  draft_write, escalation_request, list_reports, list_sources, outside_command,
  outside_write, project_delete, project_git, project_mkdir, project_read,
  project_skill_link, project_write, read_capsule, read_draft, read_large_output,
  read_report, read_skill, read_source, replan_request, result_submit,
  scope_request, write_report.

### Permission and falsification controls

Sonnet permission identity: `permission:cortex-consent:cr_f1058a652755`.
The visible card identified `outside_write` to
`/tmp/cortex-prompts-outside-xa318-claude.txt`. Approval receipt:
`gateway:bc42b500173141268c955d225d4127f7`. Luna wrote
`/tmp/cortex-prompts-outside-xa318-luna.txt` in Auto without that action.
A separate nontrivial assertion verified both files are regular non-symlinks
and contain exactly the expected 16-byte marker; receipt:
`gateway:e6ae6f9a74184338b72244ff77a70090`.

Access-validation questions were `pq_3138edf11e6a` (Sonnet) and
`pq_49c465cc7b1d` (Luna). Neither wildcard refusal was bypassed.
This run does not demonstrate denial by the native filesystem sandbox.

### Review, recovery and harness limits

Sonnet's first independent review requested changes because only the old valid
case existed. The correct strip/blank implementation alone was insufficient.
A wrongly read-only fix assignment was cancelled/replanned; a writing worker
added spaced/blank tests. Later reviews approved. Trivial external-file reads
were retained as unverified worker checks, not promoted into command evidence.
Fresh runtime verification and this worker's byte assertions are distinct.

The first GUI launch sent stdout to a closed pipe, producing a visible Electron
EPIPE modal. Its helper/Xvfb/Electron processes were stopped. Relaunch with a
dev-home log file succeeded. Initial CDP attach timed out; stale Playwright refs
and one wrong textbox-name locator also failed before corrected visible-control
actions succeeded. These are retained automation failures, not product passes.

Initially Playwright snapshots/screenshots were written inside the Sonnet
qualification repository. Runtime refused completion with
`completion_project_revision_changed`. These owned UI artifacts were moved
to separate scratch `/tmp/cortex-desktop-binding-xa318-rkgri0vw/ui-artifacts`;
the tester requested fresh review/verification, not stale acceptance or deletion
of the probe. Revision 4 passed fresh review/verification and completed.
Artifact interference is a supported harness explanation, not an assertion
that metadata alone proves the precise cause of every changed byte.

## Retained-history observations

Exact commands: `python3 -B scripts/cortex_paseo_dev.py -- logs <agent-id> --json`,
with each exact agent above and **without** `--tail`; cwd checkout.
Final observation receipt: `gateway:147783a416f0471cbe40472c7b59990d`.

| Run | Exit | Stdout bytes | Lines | Stderr bytes | Structural events / first-last identities / completeness |
| --- | ---: | ---: | ---: | ---: | --- |
| Sonnet | 0 | 11311 | 57 | 0 | unavailable |
| Luna | 0 | 6646 | 44 | 0 | unavailable |

Actual JSON parsing failed for both. Line count is **not** event count.
No raw host logs or message bodies are retained in this document.

## Automated checks and receipts

All checkout commands use cwd
`/home/igovet/Web_Projects/cortex-orch-acp`, unless stated otherwise.

| Exact command | Outcome | Receipt |
| --- | --- | --- |
| `npm --prefix integrations/paseo ci` | exit 0; audit reported 15 high vulnerabilities, not repaired here | `gateway:1a2bcde3760b46ffaa3e83a1812f5d89` |
| `npm --prefix integrations/paseo run typecheck` | exit 0 | `gateway:9cb5fc7a29c94ef7a710ccf570efdaf6` |
| `npm --prefix integrations/paseo run test:activation` | exit 0; 2 passed | `gateway:fc47735e966f425f8817b16d3b63755e` |
| `npm --prefix integrations/paseo run test:steer` | exit 0; all three scripts | `gateway:1e0795a8eba541f787972089e69bebd1` |
| `npm --prefix integrations/paseo run test:qualification-binding` | exit 0; 41 cases, no engine calls | `gateway:e709d929d9a345f185e92cca6d3ed030` |
| `python3 -B scripts/cortex_test_isolated.py --environment-root /tmp/cortex-desktop-binding-xa318-rkgri0vw --dependency-path /home/igovet/.local/lib/python3.12/site-packages tests/test_qualification_binding.py tests/test_task_inspection.py tests/test_cortex_test_isolated.py tests/test_cortex_paseo_dev.py` | first 60-second bound incomplete, exit null; same argv rerun with 180-second bound exit 0, 170 passed | incomplete `gateway:76f1c2a2b5db44a6991748327c105631`; complete `gateway:9b3ebece18374549b5dfbed477a1e650` |
| `env PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q checks/test_greeting.py` (Sonnet project) | exit 0; 3 passed | `gateway:31bd81d3b1484c5882a5e342d5169725` |
| Same exact argv (Luna project) | exit 0; 9 passed | `gateway:2217c01045c6437a885c6b27e48542ce` |

Bound inspection base command for each exact binding:
`python3 -B scripts/cortex_inspect_task.py --project <project> --session <session> --task <task> --owner-epoch 1 --limit 100`;
subsequent pages added the exact returned `--cursor`.
Both exporters used
`python3 -B scripts/cortex_paseo_dev.py binding --agent <agent> --workspace <workspace> --project <project> --timeout 10`.
Binding receipts: Sonnet `gateway:57f9e2e47bf1428abf9d27e13c34bab7`,
Luna `gateway:ef8a01f6455e4bd6bfa1cf362a7abd3c`.
Initial Sonnet busy refusal was exit 1,
`gateway:2f7e95d499be4e349111456e6360decd`, not bypassed.

Whole-suite release verification remains runtime-owned and was not rerun by this
worker. No package payload/source behavior was edited; this assignment adds
only this evidence document, so no new package stamp is required for it.

Evidence-document checks (cwd checkout):

- `env PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_project_docs.py`
  exited 0, **12 passed**; `gateway:f53a9a8d692446a0a218e3438588524a`.
- `git diff --check` exited 0; `gateway:df88f61f7b824c16932cf242646b6dfa`.
  This checks tracked working-tree whitespace; the new untracked evidence file
  was additionally read in full through the project gateway.

## Cleanup and residual work

Current GUI helper 2400050, private Xvfb 2400051 and Electron 2400054 were
observed gone after SIGTERM; receipt
`gateway:a54d06e3eba4486eb7bf0d1e9e26b9ca`.
`python3 -B scripts/cortex_paseo_dev.py -- daemon stop --timeout 15 --json`
exited 0, stopped owned PID 2386682 without force, dev home only; receipt
`gateway:6dc40e540c364f8f9a2b4c479d7b0ebd`.
A subsequent assertion established the PID absent and dev endpoint refusing
connections; receipt `gateway:485a6bfe4675403fbd08928004b85e0b`.

Disposable projects, marker files and sanitized/private scratch evidence were
left for inspection; production was not cleaned or stopped. Native-read,
filesystem-refusal and retained-log claims must not be promoted to passed from
these observations. No further paid task was launched to fill those gaps.

Reuse identity: the exact task/session/epoch and stable snapshot digests above,
the paired acceptance matrix, and the exact command receipts. Fresh evidence was
needed because these were new paid desktop tasks and Sonnet's revision changed.
Selected index `home-igovet-Web_Projects-cortex-orch-acp` matched the exact
checkout and covered the wrapper; installed host bundles required bounded direct
read-only inspection. Predecessor documentation report `r_79005ea7e2ef`
(source revision 2) supplied contract context, not live proof.


## Final-build cross-pairing follow-up — owner-requested final rerun

### Decision and authorization

**Partial live qualification, not full Part D closure.** The newly requested
Sonnet/Auto and Luna/Guarded pairings both reached bound runtime COMPLETED.
The mode-sensitive outside-write behavior, six recovery capsules, offered Cortex
tool sets, coordinator skill invocations, and consultant invocations were observed.
Genuine filesystem-denial recovery, Codex consultant native source reads,
unredacted prompt wire text, and Paseo log completeness remain unverified.

Owner answer `pq_78f32c9a3513:answer` explicitly authorized this final-build
desktop rerun; `pq_c1abfe9ffccd:answer` retained production worker routing.
This worker attempt is `xa_0eeba718b627`, parent task `t_9f911bfb173e`.
Predecessor reports `r_d49b36ab3463` and `r_eb4fe5ca65e8` (source revision 2)
supplied procedure/artifact context, not fresh live proof. Earlier observations
above are preserved; this section supersedes the earlier unexercised cross-pairings.

### Fresh environment and package identity

- Installed Paseo 0.10.3 was rechecked. Current Electron bundle supports
  `PASEO_ELECTRON_USER_DATA_DIR`, `PASEO_ELECTRON_FLAGS`, and its per-profile
  single-instance lock. Historical version claims were not reused.
- Dev endpoint `127.0.0.1:16767`, server `srv_dLC3aKF5GZxO`, owned daemon
  PID 3637797; distinct from production endpoint/PID/server. No production write,
  restart, plugin installation, account creation or credential copy occurred.
- Both native login status checks succeeded, with account/credential data suppressed.
  Dev/production routing SHA-256 remained
  `7067e5ed7913057a398a5a32e610ccab72c56bb6862c4ee57eaf8c30346c2673`.
- Plugin registration was already enabled/running at the exact absolute path
  `/home/igovet/Web_Projects/cortex-orch-acp/integrations/paseo`.
  Its fresh daemon environment selected this checkout plus dev-only Cortex XDG
  paths. Owned child observation showed the checkout's `src/cortex_acp.py`,
  and all six role-source paths selected checkout skills.
- `python3 -B scripts/cortex_package.py validate` returned
  `1.0.0+sha256.fbab0509bdd390c0`, matching the final artifact report and
  `package-identity.json`. This establishes validated source identity and actual
  launch path, not an independent in-memory binary/wire attestation.
- Electron used `~/.config/Paseo-dev`, private Xvfb display `:0`, loopback CDP
  `http://127.0.0.1:34665`, and 1440×900 viewport. Playwright drove visible
  controls; FFmpeg X11 frames were visually inspected. No screenshots, prompt
  bodies, capsules, credentials or raw host logs are embedded here.
- UI artifacts were kept outside both qualification Git repositories in
  `/tmp/cortex-final-ui-xa0eeb-srwwswbf`; no acceptance-revision interference
  from screenshot files was introduced.

### Exact final-run bindings

| Field | Sonnet / Auto | Luna / Guarded |
| --- | --- | --- |
| Disposable project | `/tmp/cortex-final-sonnet-auto-xa0eeb-jhx82tpv` | `/tmp/cortex-final-luna-guarded-xa0eeb-m2rl32qq` |
| Host agent | `fad57f12-96ed-441d-8d51-9a0698d697f9` | `d612567c-3a4c-449e-a476-c7c8cc6a235c` |
| Workspace | `wks_301812194f082c31` | `wks_ad053133d3f09561` |
| ACP session | `acps_dca917b0f26f` | `acps_7fbc8da2e917` |
| Task / epoch | `t_28cfa1229a0d` / 1 | `t_5aa36b9bae5d` / 1 |
| ACTIVE coordinator | `claude-sonnet-5`, medium, default | `gpt-6-luna`, high, agent |
| Final runtime | COMPLETED, revision 1, review cycle 1 | COMPLETED, revision 1, review cycle 1 |
| Completion UTC | 07:30:42.622596 | 07:36:39.326470 |
| Binding SHA-256 | `fcec8c0c0e063ecb6a1fe79f96903960e1bdd063a85c0e7e65b9aa6e1fe893f9` | `16af75ec3d9337fd3de43c3086a874abe10ccdee4d079af0374585cad5e47748` |
| Metadata snapshot SHA-256 | `d2761981015275da4eee49c1d4a61c850e5f41a47020bf15628640a923d256ba` | `5ae40fa18687324a3bdb954861266dec47a6f9aff061e9401b48c1b5ee9ab384` |
| Complete rows / pages | 551 / 6 | 846 / 9 |
| Final-page receipt | `gateway:5ee5e29e7e7e4ac28d2dde39649d8bfb` | `gateway:ccb1072c5239408cb248385375d498c4` |

Visible, current workspace tab identifiers supplied each exact host UUID;
the guarded binding exporter independently fetched/refetched it and checked
durable task/session/current owner. Both exports succeeded without refusal:
Sonnet `gateway:8a79cab5aefc47f8b046cebdd3be3ad5`,
Luna `gateway:2ef086a4f08e4dc8a61a326f08b02d76`.
Every metadata page followed its returned cursor and shared its run's stable
snapshot. Sonnet retained 422 task events / 67 runtime events; Luna 703 / 77.
Both retain REVIEW_STARTED and VERIFICATION_PASSED; all four worker reservations
per run were released with process_exited. This is not Paseo-log completeness
or coordinator resource-release evidence.

### Criteria-to-scenario matrix

| Criterion | Sonnet / Auto | Luna / Guarded | Boundary / limitation |
| --- | --- | --- | --- |
| Writing capsule and offered tools | Pass, scoped | Pass, scoped | Full displayed redacted recovery JSON; writer has 26 Cortex tools. |
| Read-only capsule and offered tools | Pass, scoped | Pass, scoped | Explorer has 20 Cortex tools and read_only access; native tools are separate. |
| Tool refusal followed by access_request | Pass for missing-path recovery | Pass for missing-path recovery | Literal `project_read checks/**` failed; exact `checks` reservation granted. Not filesystem containment. |
| Genuine filesystem permission denial | Unverified | Unverified | No actual filesystem-denial scenario arose; whole-project writer access is explicit. |
| Coordinator verification-coverage invocation | Pass for observed invocation | Pass for observed invocation | Actual visible `read_skill` input, not merely an assignment instruction. Display omits skill-read bodies; exact returned body/success payload is unverified. |
| Consultant invocation and Cortex set | Pass | Pass | Codex Sol; 17-tool report-only Cortex set. |
| Codex consultant native source-read capability | Unverified | Unverified | Neither consultant attempted source reading; no role-violating instruction was given. |
| Outside-project write | Pass: no tester approval/card observed | Pass: visible Allow/Deny card approved | Negative preapproval file-state control plus exact post-write bytes below. |
| Rendered timeline and final pipeline | Pass | Pass | Visually inspected Electron/X11 frames, including Guarded permission card and both Completed pipelines. |
| Bound runtime completion | Pass | Pass | Exact session/task/epoch, ACTIVE config, stable complete metadata pages, not host idle alone. |
| Paseo retained-log completeness | Unverified | Unverified | No event identity, structural event count or independent completeness signal. |
| Pristine system-prompt wire text | Unverified | Unverified | Recovery JSON redacts hashes/references and is not a system-prompt wire capture. |

### Fresh capsule observations

All six displayed capsules reported compaction level 0, Verbatim, zero omitted
entries and zero referenced fields. Sizes are displayed JSON characters.

| Run / role | Attempt | Characters | Actual confirmed native route |
| --- | --- | ---: | --- |
| Sonnet explorer | `xa_1fd67d1f96b6` | 13913 | Luna high, agent-full-access |
| Sonnet consultant | `xa_043dca3b7ac3` | 13212 | Sol high, agent-full-access |
| Sonnet writer | `xa_03b4068f29a2` | 17208 | Sol medium, agent-full-access |
| Luna explorer | `xa_484bc47f6dcc` | 12757 | Luna high, agent |
| Luna consultant | `xa_51cffe3c1809` | 12316 | Sol medium, agent |
| Luna writer | `xa_507e4c182a9a` | 14908 | Sol medium, agent |

The exact newly observed offered names match the 20/17/26 lists printed earlier
in this document, in both new runs. Fresh capsule observation receipts:
explorer Sonnet `gateway:c5c0411c1c4943ef974321d294c4457d`,
consultant Sonnet `gateway:254867b5b8d14f94856a15a1c00af92a`,
writer Sonnet `gateway:8e9c2aad426e4c85b57d829d68bf2911`;
explorer Luna `gateway:bfa25e6d31c84e88af249bbc34e69492`,
consultant Luna `gateway:e483f3ba687345c284c1d82cbd4262c1`,
writer Luna `gateway:b1d5acebc9d24546a5962fafc8f8c28c`.

The earlier capsule route-label discrepancy persists: `own_provisioning_snapshot`
contains the coordinator configuration, not the differently routed worker's
native configuration. The table uses actual worker_configuration_confirmed
events and rendered cards instead. Writers show already_project_writable;
explorers/consultants show read_only. Consultant Cortex restrictions do not
establish technical absence of Codex native tools.

### Permission and falsification controls

Sonnet's visible outside-write result reported 23 bytes at
`/tmp/cortex-final-outside-sonnet-xa0eeb.txt`. Auto was visibly selected;
the tester performed no outside approval. Exact bytes were verified before
the Luna card was approved, while the Luna target still did not exist:
`gateway:91b1a66519a34c36834d01e793fdae09`, exit 0.

Luna displayed a rendered `outside_write` confirmation for
`/tmp/cortex-final-outside-luna-xa0eeb.txt`, with the visible
Allow/Deny controls (localized as “Разрешить”/“Запретить”).
The tester clicked Allow through that card:
`gateway:9bcce7dbe71146a0b44b58145fb08595`.
Afterwards both targets were regular non-symlink files with exactly the
23-byte marker including newline:
`gateway:85254d8f1ee64fa198eb9d9cd8fc5beb`, exit 0.
A transient-card absence over every frame is not claimed; the observed Auto
path required no user approval, unlike the paired Guarded control.

Both literal wildcard reads were missing-path rejections, not permission denials.
The Luna writer explicitly reported `gateway_path_not_found`.
Sonnet's expanded access_request result was granted, complete, added `checks`,
scope revision 1, request `ar_957dd2b1dd9e`, already_project_writable:
`gateway:ee8f46eb3a584cb3bcb5c3408fa0c9e8`.
The Luna timeline and bound runtime events also retain the granted access
transition; neither refused literal read was repeated. Exact rejected read
bodies are omitted by the renderer, so the reported error code is corroborative
worker evidence, not a pristine displayed error payload.

Focused source/test reads showed normal-name preservation, whitespace trimming,
empty/whitespace-only ValueError cases in both projects, plus internal-whitespace
preservation in Luna. Original committed implementations pass only the normal
case and violate trimming/blank rejection: the non-mutating baseline
falsification command exited 0,
`gateway:661255779e9d4ceda583b46f3f7af7d2`.
This distinguishes real regression coverage from an unchanged happy-path suite.

### Commands, outcomes and harness failures

All commands below use cwd `/home/igovet/Web_Projects/cortex-orch-acp`
except the two explicitly named disposable-project pytest runs and Playwright
commands, whose cwd is the separate UI scratch directory.

| Exact command | Observed outcome | Receipt |
| --- | --- | --- |
| `npm --prefix integrations/paseo run typecheck` | exit 0, before GUI | `gateway:2105d999f1c24506abb6e5b43ed6331b` |
| `npm --prefix integrations/paseo run test:activation` | exit 0, 2 passed | `gateway:351ecc4866fe432abc1870410611d7ad` |
| `npm --prefix integrations/paseo run test:steer` | exit 0, all three scripts | `gateway:25646b731b4a4d769834b0d00d7b2e27` |
| `npm --prefix integrations/paseo run test:qualification-binding` | exit 0, 41 cases | `gateway:38ca4384924140b0aa7c0b2921ee214f` |
| `python3 -B scripts/cortex_paseo_dev.py --check` | exit 0 | `gateway:42c687d43ed94ae09dfb28cead372ceb` |
| `python3 -B scripts/cortex_paseo_dev.py -- --version` | exit 0, 0.10.3 | `gateway:037a0a4282a74eda8c0f55b32b038b6e` |
| `python3 -B scripts/cortex_package.py validate` | exit 0, final stamped identity | `gateway:d31408fc307a4733a7b890c6113810d9` |
| `python3 -B scripts/cortex_test_isolated.py --environment-root /tmp/cortex-final-ui-xa0eeb-srwwswbf --dependency-path /home/igovet/.local/lib/python3.12/site-packages tests/test_qualification_binding.py tests/test_task_inspection.py tests/test_cortex_test_isolated.py tests/test_cortex_paseo_dev.py` | first exit 2: required scratch children absent; same argv after explicit setup exit 0, 170 passed | failed `gateway:48b8d787311644f78f7188559254e61f`; passed `gateway:b76a7d35784245d9a37aad6e1704ef7f` |
| `env PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q checks/test_greeting.py`, Sonnet project cwd | exit 0, 6 passed | `gateway:1d619fdf5a9d4a2098c5560076fd1c65` |
| Same argv, Luna project cwd | exit 0, 7 passed | `gateway:ecefabc541d745b5a1b2e9fe3dc90a68` |

Retained automation failures, not product passes:
- `python3 -B scripts/cortex_package.py check` exited 2 because the supported
  action is validate; `gateway:c3922b4435c14d74b944b4cfbc888114`.
- Playwright `run-code` with statement text instead of its documented function
  argument exited 1 (SyntaxError); `gateway:c430c3136b61423a9f46d6da5c6b9c4e`.
  No UI mutation occurred; corrected function-form flow succeeded.
- The attempted X11 capture using unavailable `import` exited 1
  (FileNotFoundError); `gateway:a88f2051f7c547fbb550373ae832aae0`.
  Supported installed FFmpeg X11 capture then succeeded.
- Luna writer history retains a model-capacity diagnostic before successful
  completion; no tester model substitution was made. Sonnet's first synthesis
  shows a rejected tool card before its successful synthesis. These retained
  histories were not erased or relabeled as passes.
- A preliminary host-inspect parser selected lowercase keys, but installed CLI
  exposes capitalized summary keys and no runtimeInfo. No binding was inferred
  from that empty projection; the guarded exporter supplied the actual binding.
- Dependency installation was not rerun: the prior installation and unchanged
  dependency tree were reused; fresh typecheck and required targeted scripts passed.
  Whole-suite release verification remains runtime-owned and was not rerun here.

Binding commands use `python3 -B scripts/cortex_paseo_dev.py binding --agent
<exact host UUID above> --workspace <exact workspace above> --project <exact
project above> --timeout 10`. Metadata commands use
`python3 -B scripts/cortex_inspect_task.py --project <project> --session <session>
--task <task> --owner-epoch 1 --limit 100`; subsequent pages add only the exact
returned `--cursor`. These source-only observers never checkpointed, repaired
or directly bypassed private storage.

No-tail retained-log commands were
`python3 -B scripts/cortex_paseo_dev.py -- logs <exact host UUID above> --json`.
Both exited 0, stderr 0; Sonnet stdout 4128 bytes / 27 lines, Luna 5090 bytes /
33 lines. Neither parsed as JSON. Event count, safe first/last event identities
and completeness were unavailable; line counts are not event counts.
Sanitized observation receipt: `gateway:09c22357de164b51921e3cf81c28108e`.
Current `logs --help` was checked; no pagination/completeness signal was assumed.

### Evidence-document validation and additional retained failures

- `env PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q tests/test_project_docs.py`
  exited 0, 12 passed; `gateway:61423c05d593442b803055baf19b63a7`.
- `git diff --check` exited 0; `gateway:d796bdd418704ac9b7595cf02a464c10`.
  The evidence file is untracked, so this does not validate its own whitespace.
- Post-document `python3 -B scripts/cortex_package.py validate` initially
  exited 1 with the generic package/directory diagnostic;
  `gateway:79d63ea6d6664621be0593cc1e60b770`.
  Cause is unresolved, not attributed to the documentation. Read-only direct
  module validation then returned the unchanged final identity (exit 0,
  `gateway:2d19bb9bc43f41b6a49c35de6afb3a24`). A discriminating rerun of the
  identical CLI command also exited 0 with that identity
  (`gateway:02b0fdbcc8684dcbb05011d7eb39778f`). This retains the transient
  failure rather than hiding it behind the later pass.
- The first direct diagnostic used runpy without the script import directory
  and exited 1, ModuleNotFoundError for generate_agent_profiles
  (`gateway:801f14ffca4b4f63ae8529da254c203a`); the corrected read-only
  import produced the observation above. No source/package mutation occurred.

### Cleanup and reuse boundary

Playwright detached successfully. SIGTERM stopped owned GUI helper 3643944 and
its recorded descendants, including Electron/Xvfb; no executable owned GUI PID
remained (`gateway:6b5e338c39b3446ca6304fae9c09e314`).
`python3 -B scripts/cortex_paseo_dev.py -- daemon stop --timeout 15 --json`
exited 0 and stopped owned dev PID 3637797 without force
(`gateway:f58f45e8f42945dd8acd77a3166320bd`).
A subsequent assertion confirmed that PID absent and dev endpoint closed
(`gateway:ce04d6ee27264bf0a69d9df214cfa106`). Production was untouched;
disposable projects and evidence scratch were retained, not deleted.

Reuse identity is this final source stamp, the exact task/session/epoch and
snapshot digests above, the cross-pairing acceptance matrix, and exact check
receipts. Fresh evidence was necessary because the artifact and pairings differ
from the predecessor. No new checkout source, plugin or test behavior was edited;
only this evidence appendix changes. The source-backed PAYLOAD_ENTRIES excludes docs/, so package stamping is not repeated.
Selected index `home-igovet-Web_Projects-cortex-orch-acp` matched the checkout
wrapper's exact absolute source path; coverage had no recorded gap and matched
metadata (best-effort only). Installed host bundles required bounded direct
read-only inspection. Remaining claims must not be promoted from missing signals.
