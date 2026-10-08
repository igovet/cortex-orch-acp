# Claude ACP 0.88.0 — dev Desktop evidence, 2026-10-08

## Conclusion

**The resumed Desktop scenario passed:** a real Claude ACP 0.88.0 implementation worker completed, its independent reviewer completed, and task `t_0c750c5f00ad` reached **COMPLETED**, plan revision 2. Neither attempt emitted `assignment_failed instruction_delivery_unqualified`.

**This was not a clean first attempt or a clean check set.** The operator accidentally sent Escape during the first worker's startup; installed Paseo binds Escape to `agent.interrupt`. That attempt was cancelled with `turn_interrupted` and is retained, not accepted. A separate version-diagnostic assertion also failed because it treated sanitized command metadata as argv. A later, different check verified the full recorded launch digest against the exact 0.88.0 command. It does not erase or close that failed assertion.

No source fix, production deployment, production Paseo mutation, engine reinstall, account creation or credential copying occurred.

## Authorization, artifacts and isolation

- Owner requested **desktop** on 2026-10-08. Both route answers selected `claude-agent-acp/claude-sonnet-5`; no route substitution or escalation occurred.
- Installed Paseo CLI: **0.11.1**, not the guide's historical 0.10.2. Current CLI help exposes `--home`; current installed Electron bundle supports `PASEO_HOME`, forced `userData`, Chromium flags and per-profile single-instance locking. These were inspected before launch.
- Guarded wrapper: `scripts/cortex_paseo_dev.py`, SHA-256 `27b362c0c7fb74e0ce412ce267c53e0d88d6362c75eef5379fc3f8bd3b7ee34a`. Its production-home/profile refusal, endpoint-override refusal, dev XDG and source-root handling were read.
- Checkout HEAD: `5ee6d44b16829852e7cb3ddf3a053b3d8b6d56cd`; tested working-tree package: `1.0.0+sha256.a6cc941924a9c333`. This includes predecessor changes; it is not a claim that HEAD alone contains them.
- Dev daemon home `/home/igovet/.paseo-dev`, actual launch PID `797741`, actual endpoint `127.0.0.1:16767`. It was stopped before this assignment started.
- Electron profile `/home/igovet/.config/Paseo-dev`; GUI helper PID `799442`; private Xvfb `:2`; owned loopback DevTools `127.0.0.1:59633`.
- Disposable git project: `/home/igovet/cortex-claude088-desktop-20261008-xt6ppgzp`. Its validated project-only routing policy enables Sonnet 5 alone for every role; production and dev user policies were not replaced.
- Installed dev plugin: enabled, **running**, directory `/home/igovet/Web_Projects/cortex-orch-acp/integrations/paseo`. Install returned “already configured”; the subsequent plugin listing verified this exact current path, as allowed by the guide.
- Native Claude login status: authenticated with the existing first-party account. Only boolean/method/provider fields were retained; no account identity or credentials.
- Read-only inspection used SQLite `mode=ro` plus `query_only=ON` on the disposable project, through user-confirmed outside commands. No live workload ran against this checkout.
- Production endpoint equality was not independently probed; isolation evidence is the guarded home/profile/source-root selection and observed dev endpoint, not a claim of inspecting production state.

Consumed predecessor `r_f75982991e68`, source revision 4, SHA-256 `54da0026ccaac699770ce4a1ff77631400a99b2a906f63436d57137ecfa295db`. Its static/no-paid qualification was not treated as Desktop evidence.

## Bound outcomes

| Check | Outcome | Observed evidence |
|---|---|---|
| Plugin dependencies, typecheck, activation, steering | pass | Sequential commands exited 0; activation 2 tests; steering/question/batch scripts passed. |
| Rendered Sonnet selection | pass | Screenshot 1; coordinator confirmed Sonnet 5, medium, default mode. |
| First worker completion | fail | `xa_f84d0be06ebf`, `es_cab2913fcf95`; interrupted before configuration confirmation; screenshot 2 shows Failed. |
| Resumed implementation | pass | `xa_31d2f0abd287`, `es_885271239736`; progress event 8 confirms Claude/Sonnet 5, high, bypassPermissions; screenshot 5 shows Completed. |
| Mandatory reviewer | pass | `xa_285c61443f9a`, `es_c7d102a26377`; progress event 37 confirms Claude/Sonnet 5, medium, bypassPermissions; screenshot 6 Working, screenshot 9 Completed. |
| Actually launched adapter command | pass | Both worker process records have full digest `ab95478dd666f3db6f9af5534c974c8790549964580db1eb8cec9208f31aa96c`, equal to SHA-256 of NUL-joined current connection argv naming `claude-agent-acp/0.88.0/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js`. Coordinator's owned live cmdline independently named that same version. |
| Native agentInfo version | unverified | Delivery rows' `observed_engine_version` is null; no separate retained agentInfo version was claimed. Launch-command evidence above is distinct. |
| Instruction-delivery failure | pass | Exact-task `assignment_failed` count 0; no matching runtime failure; complete confirmed implementation/reviewer attempts exist. Absence alone was not treated as success. |
| Task end state | pass | Same exact task COMPLETED, revision 2, blocker null; one VERIFICATION_PASSED event; target README line present exactly once. |
| Initial version assertion | fail | Receipt `gateway:10e5928db779466fa99f7c544e14ef7c`, exit 1: command observation is a dict, not raw argv; regex found no version. Different later acceptance script does not close this exact-argv failure. |
| Cleanup | pass | Dev daemon stopped without force; helper absent and owned CDP closed; all exact-run registered worker, coordinator and ACP-host PIDs absent. Detached helper's numerical exit code was not retained. |
| Paid CLI / other routes | not_applicable | Only Desktop mode and Sonnet 5 were requested. No headless paid run or probe. |

Confirmation events have null `assignment_ref`. Binding here uses their exact `attempt_id` and `engine_session_ref`, joined to process records and runtime nodes, rather than inventing the missing assignment field. Recovery-capsule lookup by assignment may miss those events; this is a residual metadata concern, not fixed here.

## Interrupted canary and retry discriminator

| Context | Failure / containment | Changed causal assumption | Discriminator |
|---|---|---|---|
| Plan 1, first attempt | Operator Escape at 16:00:05 UTC; TURN_INTERRUPTED; node cancelled; README unchanged; no completion claimed. | Escape is an interrupt shortcut, not a safe picker-dismiss action. | Current installed bundle explicitly binds Escape to `agent.interrupt`; store retained `turn_interrupted`. |
| Same task, plan 2 | Explicit UI resume, no new model route or artifact; stale UI cleared by page reload. | No Escape or Stop while the worker runs. | One implementation and one reviewer completed; exact-task store and rendered headers agree. |

No later scenario was launched to work around a runtime defect. The interrupted attempt remains visible as Failed alongside the completed workers.

## Screenshots

Screenshots are crops of the actual rendered Electron window captured through its owned CDP endpoint, not fabricated DOM renders. Crops omit message/report bodies and unrelated history.

- [1: selected route before sending](claude088-desktop-shot1.png)
- [2: interrupted initial worker](claude088-desktop-shot2.png)
- [3: interrupted-turn composer / selected route](claude088-desktop-shot3.png)
- [5: resumed implementation completed](claude088-desktop-shot5.png)
- [6: reviewer working](claude088-desktop-shot6.png)
- [9: reviewer completed](claude088-desktop-shot9.png)

Number gaps are off-screen headers that were deliberately not captured.

## Commands and receipts

All commands below used cwd `/home/igovet/Web_Projects/cortex-orch-acp`. Package/profile/test operations were sequential. Long CDP scripts and paid prompt arguments are retained in gateway receipts, not copied here because this evidence excludes message bodies.

| Exact short command | Exit | Receipt |
|---|---:|---|
| `python3 -B scripts/cortex_paseo_dev.py --check` | 0 | `gateway:1b665b7b7cf546ef9b16d8ef024f26aa` |
| `python3 -B scripts/cortex_paseo_dev.py -- --version` | 0 | `gateway:6a68a82138df43639fc7ea67d85f2ded` |
| `python3 -B scripts/cortex_paseo_dev.py -- --help` | 0 | `gateway:c2c3528aa8b64e83878bd7883491d1bb` |
| `npm --prefix integrations/paseo ci` | 0 | `gateway:c89efd2cffcf4a4aad1d6f9d24fe879a` |
| `npm --prefix integrations/paseo run typecheck` | 0 | `gateway:d70cb1d06ac941d49e5745085c5029d9` |
| `npm --prefix integrations/paseo run test:activation` | 0 | `gateway:6a3742ffd4994fdb86fc62ae1a4f9168` |
| `npm --prefix integrations/paseo run test:steer` | 0 | `gateway:2a1b209747ea4bf5ab747c5df056a9b1` |
| `python3 -B scripts/cortex_paseo_dev.py -- start --timeout 60 --json` (outside) | 0 | `gateway:abc09e421cb3462c8d354333f4d70b04` |
| `python3 -B scripts/cortex_paseo_dev.py -- plugin install /home/igovet/Web_Projects/cortex-orch-acp/integrations/paseo --json` (outside) | 1, already configured | Retained installation response; not a passing check |
| `python3 -B scripts/cortex_paseo_dev.py -- plugin ls --json` | 0 | `gateway:6012d37420364746870d00edfc7acd51` |
| `python3 -B scripts/cortex_paseo_dev.py gui --xvfb` | Detached; rendered app observed | launcher `gateway:b4dafa15744645e88aea70b7111922a9` |
| `python3 -B scripts/cortex_paseo_dev.py -- daemon stop --timeout 15 --json` (outside) | 0 | `gateway:c27ab55a058f4dbb81c4ada7f2ab55c1` |
| `python3 -B scripts/cortex_paseo_dev.py -- status --json` | 0, stopped | `gateway:b8d6c697767f4a2993f33ae3b42f17ab` |
| `git diff --check` | 0 | `gateway:7932c26fbfc24f839c2d77459e17a636` |

Final metadata assertion: `python3 -B -c` with the complete Python argument below, exit 0, outside-command receipt `gateway:178331843d19455eb3e2421898827816`.

```python
from pathlib import Path; import sqlite3,json,hashlib; root=Path('/home/igovet/cortex-claude088-desktop-20261008-xt6ppgzp'); c=sqlite3.connect('file:'+str(root/'.cortex/cortex.sqlite3')+'?mode=ro',uri=True); c.execute('pragma query_only=ON'); task='t_0c750c5f00ad'; state=c.execute('select state,revision,blocker from runtime_tasks where task_id=?',(task,)).fetchone(); assert state==('COMPLETED',2,None),state; con=json.loads((Path.home()/'.paseo-dev/cortex-xdg/config/cortex/connections.json').read_text())['connections']['cn_3aec66405544']; argv=con['launch_command']; assert '/claude-agent-acp/0.88.0/node_modules/' in argv[1]; digest=hashlib.sha256('\0'.join(argv).encode()).hexdigest(); expected={'xa_31d2f0abd287':'es_885271239736','xa_285c61443f9a':'es_c7d102a26377'}; confirmations=[];
for event_id,payload in c.execute("select id,payload_json from task_events where task_id=? and event_type='progress'",(task,)):
 d=json.loads(payload);
 if d.get('kind')=='worker_configuration_confirmed':
  assert d['attempt_id'] in expected and d['engine_session_ref']==expected[d['attempt_id']]; assert d['engine']=='claude-agent-acp' and d['model']=='claude-sonnet-5'; confirmations.append((event_id,d['attempt_id'],d['engine_session_ref'],d['model'],d['parameters']))
assert len(confirmations)==2; records=list(c.execute("select attempt_id,engine_session_ref,launch_command_digest,terminated_at from attempt_process_registry where task_id=? and role='worker'",(task,))); assert len(records)==2;
for a,s,h,end in records: assert expected[a]==s and h==digest and end is not None
fails=list(c.execute("select id,payload_json from task_events where task_id=? and event_type='assignment_failed'",(task,))); assert not fails; assert all('instruction_delivery_unqualified' not in r[0] for r in c.execute('select payload_json from runtime_events where task_id=?',(task,))); assert all(r==('claude-agent-acp','claude-sonnet-5','authorized') for r in c.execute('select engine,model,decision from attempt_route_audit where task_id=?',(task,))); assert (root/'README.md').read_text().splitlines().count('Claude 0.88 desktop check complete.')==1; assert c.execute("select count(*) from runtime_events where task_id=? and type='VERIFICATION_PASSED'",(task,)).fetchone()[0]==1; print(json.dumps({'task':task,'state':state,'confirmed_workers':confirmations,'full_launch_digest':digest,'worker_process_records':records,'assignment_failed_count':len(fails),'instruction_delivery_unqualified':False,'readme_line_count':1,'verification_passed_events':1,'interrupted_initial_attempt_retained':True})); c.close()
```

Cleanup: `python3 -B -c` exact PID-bound inspection/termination programs retained under `gateway:9948937093c5462aad9666f15c47e987` and `gateway:22edb952d9da402ebcb3c54f282beca9`, both exit 0. These observed owned helper termination/CDP closure and all task-registered processes absent.

## Failed preparation and limitations

- Initial copied-policy validation exited 1 before any write; a minimal validated project-only policy was then written. No user policy repair was attempted.
- Two sandbox live-SQLite inspections failed to open the database; subsequent permission-approved outside read-only inspections succeeded. One schema query used `event_type` where `runtime_events` has `type`; corrected inspection succeeded. These are diagnostic failures, not missing product passes.
- A CDP selector initially found hidden retained controls; visible controls were subsequently selected. A malformed newline expression did not execute its intended click. No route substitution occurred.
- The failed version assertion remains an open failed check, not a passing rerun. Full launch-digest verification subsequently establishes the launch version separately.
- `npm ci` reported 21 existing vulnerabilities (5 moderate, 15 high, 1 critical); no audit repair or package changes were authorized.
- No full repository suite, production qualification, native-tool enforcement audit or paid CLI was run. This assignment changed evidence only; final repository verification belongs to the runtime.
- This evidence is outside the runtime payload inventory; package stamp was not changed.

## Blast radius and handoff

Repository changes are this evidence page and six PNG crops only. External effects were the owner-requested paid Desktop turns, dev-only plugin/start/stop operations and one disposable project. Native accounts and production installations were left unchanged. Both dev services are stopped; the disposable project's metadata and private logs remain outside repository docs. No secret permissions or credentials were added.

Next action: independent review should evaluate the successful resumed scenario while retaining the interrupted attempt, the failed diagnostic and the null-assignment confirmation concern. This evidence does not declare the entire release qualified.
