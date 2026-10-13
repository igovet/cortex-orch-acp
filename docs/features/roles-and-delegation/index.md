# Roles and delegation

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed.


The coordinator proposes a structured plan and submits synthesis. The runtime
validates the plan, reserves ready nodes, selects the configured role route,
provisions each engine session, delivers a bounded assignment and schedules review.
There is no public model-controlled spawn, effort selection or acceptance API. A coordinator plan requires a constrained `model` recommendation and may include `model_reason`; `list_route_options` provides a read-only live menu. Runtime policy makes the final route and effort decision. See [automatic routing](../routing-and-fallback/index.md#automatic-worker-routing-tiers-model-hints-and-ranking).

Critical rules require an exact qualified instruction-delivery channel.
Claude 0.88.0 now has its own inspected registration; dispatch supplies the
same native-tool restriction metadata as the earlier registered releases.
Unregistered Claude versions cannot displace registered installs, and a
non-retryable delivery refusal makes the task's engine/version unavailable
across models for critical-role dispatch. This is a delivery qualification
boundary, not evidence of effective model/tool enforcement. See [Claude
support and limits](../claude-engine/index.md#current-support-0880-2026-10-08). The 2026-10-08 owner-requested Desktop run on 0.88.0 is bounded evidence, not clean qualification: Desktop evidence.

All 23 worker role instructions remain available as canonical skill sources,
composed from role specializations and the shared report protocol. The redundant
`agents/*.toml` profile files and generated reference copies have been removed;
the developer generator renders from canonical `skills/worker-*/SKILL.md`
sources. Instructions describe intellectual work and evidence; they are not an
authorization mechanism. Workers publish immutable Markdown reports and submit
structured results through their authenticated channel. They receive explicit
predecessor report references, not an automatic copy of the parent chat. The
build-verification role is retained among the 23 but is runtime-reserved: it is
not plannable or dispatched because command verification is executed by the
runtime itself. The runtime directive selects the coordinator's user-facing reply
language from the chat language (English or Russian); worker communications and
reports remain English. User-facing structured question text and options are
the exception: the question tool contract requires the current turn's
`user_question_language` (`en` or `ru`), supplied on every worker turn.
The runtime relays strings verbatim and does not translate or deterministically
verify their natural language.

The profile catalogue (`profiles.json`) declares `access_class` as `write`,
`read_only` or `consultant` for each profile. The generator selects the matching
canonical verification block from `agent-sources/verification-*.md`; ordinary
workers share `worker-protocol.md`, while consultants use `consultant-protocol.md`.
A drift test asserts exact equality between catalogue non-writing roles (including
the consultant alias) and the runtime's literal `READ_ONLY` set. This metadata
selects instructions; it does not replace runtime authorization.

Worker report examples, predecessor evidence and command-result requirements are
specified once in the shared protocol; specializations retain only domain-specific
handoff content. Reviewers default to inline immutable report publication with
an explicit verdict and the complete shared decision brief; the general report
example does not impose a length limit. Owned drafts remain available for genuine
incremental editing and other roles. See [reviewer handoff](../orchestration/index.md#reviewer-handoff).
Publication transport arguments remain in tool descriptions,
not profile instructions. `tool-discipline` is common call/evidence guidance;
`tool-discipline-worker` contains project reservations and command receipt duties.
The coordinator reads `skill-management` for activation and approved proposals;
workers read `skill-authoring` for creation, revision, import and guarded links.
Harvest scope assessment is coordinator-only `knowledge-harvest-scope`, while
worker-only `knowledge-harvest` owns census and the canonical coverage contract.
One shared-skill allowlist supplies read authorization and the role's `read_skill`
enum; coordinators cannot read worker-only procedures, and consultants cannot read
these project procedures. All 23 profiles and their access classes remain intact.

## Coordinator-to-worker location hand-off

When the coordinator has actually looked up relevant source locations, the
assignment objective can include a compact **Known locations** block with
`path:line`, symbol and a one-line relevance note. It is optional and must not
be manufactured through deep investigation: the coordinator still orients but
does not investigate just to populate hints. Workers treat locations as
staleable pointers, read the current source before editing, and begin there.
They broaden discovery or consult project/feature indexes when the hints are
insufficient or the change affects a contract or documentation surface. This
passes specific findings, not coordinator transcripts or engine sessions.

The attached code-index prompt can also provide the pre-resolved indexed-project
name for the current root; use it directly and list projects only as fallback.
These prompt-level hand-offs do not validate location freshness, enforce a
worker's choices, or establish reduced latency or improved model outcomes.
Structured, digest-bound locations (F3) and discovery-time measurement (F4)
remain deferred unless the prompt-only approach proves insufficient and the
measurement is observable. Workers use an already supplied matching code-index
project directly; project listing is a fallback for absent or ambiguous matches.
Review reuse is based only on exact assignment, artifact/report revisions and
check or command-receipt identities actually observed, not a fixed key absent
from task inputs. A host-injected inactivity nudge is host-authored, an accepted
exception to the worker's no-narration-between-tool-calls convention; workers
must not imitate or rely on it.

## Instruction delivery

Read-only roles (`orchestration.contracts.READ_ONLY`: architect, ux-designer, code-reviewer, senior-consultant, explorer, planner, security-auditor, accessibility-auditor, database-architect, performance-engineer and the runtime-reserved build-verification profile) never change the project, whatever scope a plan requests. `plan_submit` therefore refuses a read-only task whose scope names a path that does not exist yet (`read_only_scope_not_writable`): such a path can only be an output the worker could never write. On 2026-09-28 a plan gave seven Opus ux-designer tasks an output file each; every worker did the work and then failed to save it against the read-only project. The task returns content in its report and a dependent writing task creates the file.

`INLINED_SKILLS` are delivered in full with the role instructions because their case arises in almost every session: `tool-discipline`, `content-safety` and `coordinator-communication` for the coordinator; `tool-discipline`, `tool-discipline-worker`, `content-safety`, `report-publication` and `code-and-evidence` for ordinary workers (consultants omit project/command discipline). The text is exactly what `read_skill` returns for each id. Naming them was not enough: on 2026-09-28 Claude sessions given the same named list never called `read_skill`, while Codex sessions did. `NAMED_SKILLS` names the remaining situational skills with their triggers; a model requests those through `read_skill`, so they are not pasted into every prompt. `situational_skill_triggers` supplies the same action-bound catalogue to the role text, the `read_skill` tool description, and its `skill` selector description. The model is instructed to call `read_skill` before the matching action, not to pre-load all skills: editing documentation made wrong by a behavior change calls for `documentation-sync`; starting a browser or server session calls for `interactive-resources`. Inlined skills are excluded from this catalogue. The `read_skill.skill` selector advertises an `anyOf`: a role-scoped enum for named Cortex shared/reference skills, plus a separate path branch for an exact project/user `SKILL.md` listed in the current session. The runtime checks that path against the session grant; it is not restricted to the named-skill enum. Enum membership alone is not a trigger. Coordinator channels include the coordinator-only shared skills identified by `COORDINATOR_ONLY_SHARED_ROLE_IDS` (`cortex-control`, `coordinator-communication`, `skill-management` and `knowledge-harvest-scope`); worker and consultant channels do not. Consultants are not prompted to load mutation-oriented skills. The runtime does not read situational skills on the model's behalf. Automated prompt/schema tests establish delivery, not model compliance on a true trigger.

Reference ids are scoped separately: workers can read the worker references (`report-publication`, `code-and-evidence`, and `interactive-resources`), coordinators can read their coordinator references, and both can read the example catalogue (`index` and the `*-example` ids). The runtime checks the channel role and the enum; prompt text only explains when an available skill may be useful and grants no authority. Cortex MCP `read_report`, `read_skill`, draft/publication, `command_run` and result tools are distinct from external code-index MCP tools and native workspace aids; only the Cortex command receipt supports a passed worker check. The `documentation-sync` trigger covers coordinator documentation-impact planning as well as affected edits. The 2026-09-28 live CLI backend worker wrote only code/tests, while a Codex writer handled README updates, so that run did not test Claude worker compliance on a genuine documentation-edit trigger.

Recovery capsules derive `allowed_mcp_tools` from the same role/stage-specific offered tool definitions used for the channel's `tools/list`; they do not include a `native_tools_enabled` field because the runtime cannot establish a reliable native-tool inventory. The capsule's environment guidance likewise distinguishes writers, read-only assignments and the senior consultant instead of granting a generic machine-readable writer instruction.

### Senior consultant evidence and tool boundary

The senior consultant works from its consultation packet and retained reports,
not project source or an attached code index. Its Cortex tool offer retains
report/source-packet reads (`read_report`, `list_reports`, `list_sources`,
`read_source`, `read_capsule`, `read_large_output`), skill reads, owned report
draft/publication and result submission, questions and request tools. It is not
offered `project_read`, `project_git` or `command_run`; attempts to read project
source through those routes fail with `gateway_read_denied`. No external MCP
servers are attached. This is not a complete native-tool sandbox: Claude's
consultant profile disallows native file/search/edit tools, and Codex's own
`shell_tool`/`view_image` features are likewise forced off for every Cortex-
launched Codex session, including the consultant's (`engine.process`'s
per-process launcher and session-config patch; verified against the installed
Codex CLI 0.154.0's `features list`). Codex native `apply_patch` has no
supported off switch and a separate `unified_exec` feature was found to have
no supported off switch either (neither `-c features.unified_exec=false` nor
`--disable unified_exec` had any effect against that same installed binary);
whether either still offers Codex a residual native read path within its
read-only engine jail is not established without a live qualification check.
Therefore the packet/report-only rule is the Cortex tool and prompt boundary,
not a claim that every engine is technically unable to inspect source.

The runtime also enforces observable situational triggers at worker `result_submit` (`orchestration.skill_gate`), the same for both engines. Codex edits through its native `apply_patch`. Claude's native Edit, Write and Read are disallowed in Cortex sessions (`CLAUDE_GATEWAY_ALTERNATIVES` in `src/cortex_runtime/engine/claude_adapter_acp.py`), so Claude edits only through Cortex `project_write` or `project_edit`. The shared observation boundary is the project tree: the runtime retains each shared-root attempt's pre-attempt tree manifest (`attempt_revisions` phase `worker_start:<attempt_id>`) and observes the tree again at submission. `SKILL_GATES` is the one table of these triggers; today it holds `documentation-sync`: documentation (`*.md` in a `docs` directory, any `README.md` or `SECURITY.md`) changed together with a behavior file (anything except prose formats, bytecode caches and verified engine housekeeping; directory entries alone never count). When the attempt's changes match and the attempt has no retained successful `read_skill` of that id (each worker read is recorded as a `skill_read` observability event for that attempt only), the result is refused with the recoverable code `situational_skill_required` before anything is recorded or published. The error names the skill and example paths and tells the worker to read it, re-check the edits and submit again; the refused `request_key` may be reused. The refusal counts in the existing `submission_rejection` events. A request already recorded replays without being gated again. Only roles whose situational catalogue names the skill are gated: read-only roles, review stages and the consultant are not. Attribution uses only changed paths in this node's declared scope or its own successful gateway write/delete events. Sibling writers' reserved paths are excluded unless this attempt has its own gateway write receipt for that path; read-only/review scopes do not claim ownership. Successful sibling gateway writes outside their reservation are also excluded from this worker's delta, unless this attempt has its own write receipt. Ambiguous overlapping native edits are not charged to either worker. Missing complete manifests or node scope stay explicitly unobservable, never an inferred pass. Successful gateway `project_write`, `project_edit` and `project_mkdir` calls also return early `required_before_result_submit` skill/reason hints from the shared-tree trigger, attributed the same way as the result gate (F2 fix: a sibling's unrelated change -- e.g. its own docs/** edit -- is never hinted to a worker whose own changes do not match the trigger; no hint at all when attribution itself is unavailable). A sibling-only trigger can yield a non-error reminder, but cannot refuse this worker's result. A later successful `read_skill` clears hints on subsequent writes; native edits still rely on the result gate. Hints are advisory: a failure anywhere in the hint computation, including database connection acquisition or the worker node lookup, returns `skill_hint_unavailable` without reclassifying a committed mutation as failed. The successful mutation receipt (including that diagnostic) is retained for identical retries; the result gate remains authoritative. The gate makes the read observable and required; it does not prove the model applied the skill well.

An independent reviewer is a fresh runtime assignment. Its verdict controls the
review/fix loop, while actual command verification belongs to the runtime. Route policy is configured by the user and resolved by the runtime for each role; per-node model hints are validated recommendations, not authority.

Declared scopes coordinate the agreed work areas of parallel writers; the
runtime grants a writing worker project-wide gateway and jail write access, with
private and user-protected paths still refused. Writers should reserve a project
path outside their agreed work area with `access_request` before changing it; that
request does not widen an OS jail. Writer conflicts receive bounded delayed
runtime rechecks before a grant or denial without a user question; policy, path
and read-only refusals are immediate runtime decisions. Only a fresh runtime
predicate can grant access, never a user answer. The worker question contract
reserves questions for genuine user decisions, not missing tool access. Outside-project effects use `outside_write` or
`outside_command` and, in Guarded mode, require the user's accepted permission
card. In Development mode `outside_write` needs no card unless it targets
protected control-plane state, while `outside_command` still requires one. Auto mode intentionally approves them without a per-action card, including
when no client is connected; this is the owner's selected behavior. Guarded
requires an accepted permission-card response and fails closed on no response,
denial, cancellation or timeout. Read-only
roles and review stages have no project write grant. See
[containment](../permissions-and-containment/index.md) and
[verification status](../../project/verification.md). Prompts grant no authority.
