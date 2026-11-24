# Roles and delegation

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed. The former Python runtime was removed on 2026-11-16: references to its modules and tests were dropped, and statements that cited them describe behavior carried from that implementation that has not been re-verified against the Rust runtime.


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
`agents/*.toml` profile files have been removed. The developer generator
 renders the `agent-sources/`
specializations and shared protocols into the canonical `skills/worker-*/SKILL.md`
files and their `references/` copies; its `check` action (`scripts/cortex-xtask.sh profiles check`) asserts the 23 outputs
and that each `profiles.json` `select_when`/`avoid_when` equals its source's
"When to use this profile" bullets. Generated YAML frontmatter and footer comments stay in those files for native skill loaders and are removed from the portable body that engines receive. Instructions describe intellectual work and evidence; they are not an
authorization mechanism. Workers publish immutable Markdown reports and submit
structured results through their authenticated channel. They receive explicit
predecessor report references, not an automatic copy of the parent chat. The
build-verification role is retained among the 23 but is runtime-reserved: plan
intake refuses it and no runtime path dispatches it, because the runtime runs the
plan's verification commands itself (as the former runtime did). Its profile,
instructions and tool catalogue stay complete and are exercised on a stored node
by the role test; it is dispatched only if a future runtime stage needs a
model-run build check.

The assignment brief (the advance step's assignment, which is the worker's prompt)
carries, never truncated away: the objective and `scope`; `acceptance`, the plan's
verification commands the runtime runs after an approving review; `constraints`,
the owner's messages to the task (bounded, with size and sha256 when long), the
write denials in force and the access mode; `predecessor_reports`, every report
of the node's dependencies (for the consultation, every report of its packet)
with its id, node, role, title, size and page count; `report`, the
runtime-selected report skeleton (see [Markdown reports](../markdown-reports/index.md#report-structure));
and `user_question_language`. A review gets its `review_packet` instead, whose
`predecessor_reports` lists only the reports that review must read to the last
page: the work of its round (after an earlier completed review of the same plan
revision, the fixes scheduled since and that review's own report, so its findings
are neither re-raised nor lost; for the first review of a revision, the work that
recorded project writes or changes, or all of it when none did). Earlier fix
rounds and reviews, work without recorded writes and earlier plans are listed in
`optional_reports` (newest first, at most 24, the rest counted in
`optional_reports_more`), and `predecessor_reading` states the rule. The required
set does not grow with replans or fix rounds. A required report with no published
edition, or one the runtime itself cannot read (anything but a busy lock), is
not required: the packet lists it in `unavailable_reports`, and the accepted
review result keeps it in `predecessor_reports_unreadable` with its reason, so
the refusal can always be cleared.

Language: everything another worker or the coordinator reads is English (worker
reports and their headings, summaries, briefs, worker messages and the
coordinator's instructions to workers). Everything the owner reads directly is in
the owner's language: the coordinator's messages and questions to the owner, the
final result text and its not-verified list, a worker's question to the owner
(question text and option labels) and the runtime's own stop questions. The owner's
language is the task's `en`/`ru` setting, otherwise the language of the task's
goal; the runtime records it in `task_language` at task start and at every turn,
states it in every turn's route instructions per role, and gives workers
`user_question_language`. The coordinator composes the owner-facing text from the
English reports in the owner's language, including owner-facing report prose.
Submitting a plan or reading English tool output is not a reason to repeat the
plan or switch languages. The runtime relays model-authored strings verbatim and
does not translate or deterministically verify their natural language.

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
enum; coordinators cannot read worker-only procedures, consultants cannot read
these project procedures, and read-only roles cannot read the file-changing
`skill-authoring` and `knowledge-harvest` procedures. All 23 profiles and their
access classes remain intact.

## Capability-based delegation

The canonical `skills/orchestrator/SKILL.md` distinguishes answering from doing:
the coordinator can answer simple informational questions using granted reads.
Changes, substantial investigation, and operations requiring capabilities outside
its role go into a specialist objective in a structured plan, even if phrased as
a simple read. This is general guidance, not server-name or request-keyword
routing. Worker command and outside-project permission routes do not become
coordinator tools. Missing coordinator tools do not establish machine-wide
unavailability; unconfigured or unknown worker capability must stay explicit.
A refusal is not a discovery gap: do not retry it via another tool, engine or
worker. Additional access still requires runtime validation.

`worker_capabilities_section` renders a separate
configured-worker MCP summary from the session's source inventory. It applies
`host_mcp_servers` worker/engine admission filters, then shows read-only-tool
intersection and writing-tool grants for each eligible server and engine filter.
The consultant receives no external servers. Engine filter identities are not a
claim that a route is installed or available; the live route menu and runtime
policy decide dispatch. An absent source inventory is labeled unavailable;
an empty or fully filtered inventory reports no eligible configured servers.
These cases must not be presented as a successful external read.

Only sanitized server/tool identifiers are shown: at most 24 tool names per access
class and a 12,000-character row budget, with explicit omitted-name/row counts
and a small fixed explanatory footer. No command, arguments, environment values,
filesystem grant paths, descriptions or tool schemas are copied. The summary is
a startup snapshot of configuration, not observed tool discovery or successful
invocation, and does not perform full proxy admission checks. Workers inspect
their actual live tool surface and report refusals or missing configuration.

`EngineSessionManager.create_session` delivers this summary to coordinators even
when they have no attached external servers; Codex and Claude receive it through
the existing instruction metadata. OpenCode receives it through its launch
prompt. Workers still receive only their own attached-server section. The
metadata-only `external_mcp_inventory` receipt continues to describe only the
current session's forwarded servers, not worker capabilities or model use.

The same coordinator section includes a built-in Cortex discovery roster derived
from current `tool_definitions` offers for a read-only investigator, writing
specialist and consultant. It is independent of external MCP attachments and
lists only relevant operation names, not copied schemas. A worker can use an
offered `command_run` for authorized bounded reads in its actual command namespace;
Cortex has no separate outside-read/search tool. Read-only/Guarded commands have
private scratch at `/var/tmp`, not host data. A missing file there proves only
command-environment visibility. For authorized host-only reads/searches, use the
advertised `outside_command` consent route through a writing specialist with a
bounded read-only objective, or a granted external reader. A read-only investigator
cannot escalate to that command route; it reports the capability gap for runtime
replanning. Neither remounting nor moving a fixture establishes the original read.
`project_read` remains project-only. Role, mode, protected paths and owner
constraints still apply. `access_request` handles a writing worker's
assignment-local additional project work paths, not outside grants or remounts.
It reserves write paths only, so it is served to writing workers alone: a
read-only role or any review already reads the whole project, could never be
granted a write, and reports a missing access as a limitation (the former runtime
refused it there with `read_only_role`). Exceptional outside
command/write routes belong to writing workers, never to read-only escalation
or denial bypass.

Unknown external source locations become worker discovery objectives. The
coordinator orients with its permitted reads, then proposes a suitable investigator
rather than demanding pasted data because its own tools cannot reach the source.
A Paseo session UUID identifies host history, not a current-project Cortex report;
`read_report` cannot resolve that host identity. Ordinary workers receive canonical
shared guidance to inspect actual installed help, storage or export mechanisms
read-only, validate the authorized exact-session binding, and report bounded
page/byte/event coverage and truncation. A reader that may load, resume or create
an agent does not satisfy a no-resume investigation. If no safe mechanism is
observable, evidence remains unavailable; an export request is a last supported
option, not the starting assumption. No host path or API is assumed.

The former Python no-paid regressions exercised
arbitrarily named multi-server grants, engine/access filtering, sanitized bounds,
missing inventory, built-in discovery without external attachments, unknown
locations, Paseo UUID/report separation, composed instructions, unchanged role
tool offers, real cwd validation, command refusal and scripted plan-to-worker handoff. Existing direct-answer tests retain
informational reads without workers. These prove delivery and deterministic
runtime behavior, not that a paid model will choose delegation correctly.

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

The attached code-index prompt and the inlined `code-and-evidence` reference both
tell workers to call `list_projects` once and select the project whose root
matches the workspace; the runtime passes no pre-resolved project name. Only a
role granted `index_repository` is told to attempt one bootstrap.
These prompt-level hand-offs do not validate location freshness, enforce a
worker's choices, or establish reduced latency or improved model outcomes.
Structured, digest-bound locations (F3) and discovery-time measurement (F4)
remain deferred unless the prompt-only approach proves insufficient and the
measurement is observable.
Review reuse is based only on exact assignment, artifact/report revisions and
check or command-receipt identities actually observed, not a fixed key absent
from task inputs. A host-injected inactivity nudge is host-authored, an accepted
exception to the worker's no-narration-between-tool-calls convention; workers
must not imitate or rely on it.

## Instruction delivery

`composed_instructions()` requires every applicable entry in
`INLINED_SKILLS` to load with a nonempty portable body. It no longer silently
omits unreadable required skills. Failure raises a non-retryable
`PortableError` (`required_skill_unavailable`) whose remediation names the
receiving role, skill id and underlying error code/remediation; the original
error remains chained. `build_instruction_bundle()` propagates the same error.
Consultant exclusions remain unchanged. Situational entries are named in the
catalog, not eagerly loaded, so an unavailable situational source does not
block composition.

Worker provisioning composes instructions in `_step_engine_session` before
recording an engine-open intent or calling the opener. Its existing
`_run_steps`/`_fail` path records failed status and error code, retains the
engine-session step and clears the pending channel secret. The existing
`OrchestratingRuntime._provision_and_prompt_inner` handler emits
`assignment_failed` with the specific diagnostic instead of `assignment_ready`.
This uses the existing provisioning error/step reporting, with no new lifecycle
or permissions mechanism. After repairing the source, a failed delegation
requires a fresh request; the same failed request is not reopened.

Requirements intake and design workflow are coordinator-only situational skills.
The five design-* skills have stage-specific worker allowlists in
`DESIGN_SKILL_ROLES`, used for both delivery and read authorization;
backend and consultant channels do not receive them. See
[requirements intake](../requirements-intake/index.md) and the
[UI/UX design workflow](../design-workflow/index.md) for intake, document
ownership and screenshot evidence responsibilities.


Read-only roles (`READ_ONLY`: architect, ux-designer, code-reviewer, senior-consultant, explorer, planner, security-auditor, accessibility-auditor, database-architect, performance-engineer and the runtime-reserved build-verification profile) never change the project, whatever scope a plan requests. `plan_submit` therefore refuses a read-only task whose scope names a literal path (no `*`, `?` or `[`) that does not exist yet (`read_only_scope_not_writable`): with no writer behind it, such a path is an output the worker could never write. On 2026-09-28 a plan gave seven Opus ux-designer tasks an output file each; every worker did the work and then failed to save it against the read-only project. The task returns content in its report and a dependent writing task creates the file. A nonexistent path is accepted when a writing task of the same plan whose scope overlaps it (the scheduler's the runtime) is ordered with the read-only task, transitively: the read-only task depends on it (a reviewer of the directory its predecessor writer creates), or it depends on the read-only task (the remedy above). An unordered parallel writer does not count, since the read could precede the write. The refusal lists every remaining path in `violations`, so one correction fixes them all. On 2026-10-14 three coordinator plans in one design project were refused on their first attempt for exactly those writer-covered reviewer and art-direction paths.

`INLINED_SKILLS` are delivered in full with the role instructions because their case arises in almost every session: `tool-discipline`, `content-safety` and `coordinator-communication` for the coordinator; `tool-discipline`, `tool-discipline-worker`, `content-safety`, `report-publication` and `code-and-evidence` for ordinary workers (consultants omit project/command discipline). The text is exactly what `read_skill` returns for each id. Naming them was not enough: on 2026-09-28 Claude sessions given the same named list never called `read_skill`, while Codex sessions did. `NAMED_SKILLS` names the remaining situational skills with their triggers; a model requests those through `read_skill`, so they are not pasted into every prompt. `situational_skill_triggers` supplies the same action-bound catalogue to the role text, the `read_skill` tool description, and its `skill` selector description. The model is instructed to call `read_skill` before the matching action, not to pre-load all skills: editing documentation made wrong by a behavior change calls for `documentation-sync`; starting a browser or server session calls for `interactive-resources`. Inlined skills are excluded from this catalogue. The `read_skill.skill` selector advertises an `anyOf`: a role-scoped enum for named Cortex shared/reference skills, plus a separate branch for a project skill given by name, directory or `SKILL.md` path below the project skill directories (see [project skills](../unified-system-prompt/index.md#project-skills)). The runtime resolves it read-only inside the project for roles served `project_read`; a Cortex id always selects the Cortex skill. Enum membership alone is not a trigger. Coordinator channels include the coordinator-only shared skills identified by `COORDINATOR_ONLY_SHARED_ROLE_IDS` (`cortex-control`, `coordinator-communication`, `skill-management`, `knowledge-harvest-scope`, `requirements-intake` and `design-workflow`); worker and consultant channels do not. Consultants and read-only roles are neither named nor offered the file-changing `skill-authoring` and `knowledge-harvest` procedures; read-only roles keep `documentation-sync` for impact assessment. Always-inlined skills carry no trigger. The coordinator's `pipeline-publication` reference is named "before submitting the final synthesis", and its former `recovery` reference was folded into `context-compaction`. The runtime does not read situational skills on the model's behalf. Automated prompt/schema tests establish delivery, not model compliance on a true trigger.

Reference ids are scoped separately: workers can read the worker references (`report-publication`, `code-and-evidence`, and `interactive-resources`), each design stage owner can read the design data references of its parent design skill (for example `design-colors`, `design-critique` or `design-defect-checklist`), and every design role can read `design-banned-patterns`, the worker-readable copy of the coordinator-only `DESIGN` template's "Banned AI-tell patterns" section (a test keeps the two identical), coordinators can read their coordinator references (including `question-banks`, `agreed-spec`, `design-intake`, the `PRODUCT`, `DESIGN` and `SCREENS` templates and `visual-review`), and both can read the example catalogue (`index` and the `*-example` ids). The runtime checks the channel role and the enum; prompt text only explains when an available skill may be useful and grants no authority. Cortex MCP `read_report`, `read_skill`, draft/publication, `command_run` and result tools are distinct from external code-index MCP tools and native workspace aids; only the Cortex command receipt supports a passed worker check. The `documentation-sync` trigger covers coordinator documentation-impact planning as well as affected edits. The 2026-09-28 live CLI backend worker wrote only code/tests, while a Codex writer handled README updates, so that run did not test Claude worker compliance on a genuine documentation-edit trigger.

Recovery capsules derive `allowed_mcp_tools` from the same role/stage-specific offered tool definitions used for the channel's `tools/list`; they do not include a `native_tools_enabled` field because the runtime cannot establish a reliable native-tool inventory. The capsule's environment guidance likewise distinguishes writers, read-only assignments and the senior consultant instead of granting a generic machine-readable writer instruction.

### Senior consultant evidence and tool boundary

The senior consultant works from its consultation packet and retained reports,
not project source or an attached code index. Its Cortex tool offer retains
report/source-packet reads (`read_report`, `list_reports`, `list_sources`,
`read_source`, `read_capsule`, `read_large_output`), skill reads, owned report
draft/publication and result submission. It is not offered `command_run`, `project_read`,
`project_git`, `ask_question`, `access_request`, `replan_request`, `scope_request` or
`escalation_request`. Dispatch refuses the question, access and three lifecycle
requests, so a consultation cannot cancel workers or recursively escalate; attempts
to read project source through those routes fail with `gateway_read_denied`. The consultant is
reached only through `escalation_request`: a plan that names it is refused
(`consultant_requires_escalation`). The runtime builds its packet from the reports the
escalation names, then the escalated revision's reports, each with its SHA-256 and recorded
artifact versions. Missing evidence becomes a limitation in the consultant's report, and the
consultant asks the user nothing. Its protocol states English-only
communication, and its turn wrapper omits the question-language exception that
other worker turns carry. No external MCP
servers are attached. This is not a complete native-tool sandbox: Claude's
consultant profile disallows native file/search/edit tools, and Codex's own
`shell_tool`/`view_image` features are likewise forced off for every Cortex-
launched Codex session, including the consultant's (the runtime's
per-process launcher and session-config patch; verified against the installed
Codex CLI 0.154.0's `features list`; later pages re-observed 0.166.0). Codex native `apply_patch` has no
supported off switch and the separate `unified_exec` flag has none either
(neither `-c features.unified_exec=false` nor `--disable unified_exec` changed it).
With the launcher's flags the local 0.166.0 CLI inventory declares neither
`exec_command` nor `write_stdin` ([details](../codex-engine/index.md#codex-shell-tool-inventory-and-unified_exec));
whether `apply_patch` or a custom launch command offers a residual native path
within the read-only engine jail is not established without a live qualification check.
Therefore the packet/report-only rule is the Cortex tool and prompt boundary,
not a claim that every engine is technically unable to inspect source.

The native runtime does not enforce situational skill triggers at worker `result_submit`: it retains neither a per-attempt tree manifest nor a record of which skills an attempt read (`read_skill` is a read that records nothing), so a refusal such as `situational_skill_required` cannot be grounded in observable data and is not issued. Skill use rests on the role instructions and the `read_skill` catalogue of situational triggers (for example `documentation-sync` before assessing documentation impact), and on independent review. The earlier per-attempt manifest gate belonged to the Python runtime.

Every worker finishes its own assignment within its role and permissions. A
`result_submit` that leaves something the worker can still do is refused
correctably with the outstanding items listed, and the same session continues;
a failure (`assignment_outcome: "failed"`) is accepted only with a structured
`blocker` naming the dead end (`missing_access`, `owner_decision`,
`other_role_work` or `tool_unavailable`) and its concrete detail, which reaches
the coordinator with the handoff. A turn that ends without a result is asked once
more in the same session. No role is redispatched for the same revision and
objective because an earlier attempt left work undone; only a lost process is
retried within the bounded retry window.

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
reserves questions for genuine user decisions, not missing tool access.
Outside-project effects use `outside_write` or `outside_command`. In Guarded
and Development, each eligible action requires owner approval through a
permission card, including ordinary outside writes. Without approval, the
action does not run. In Auto, the runtime approves eligible actions without
a per-action card, even when no client is connected. Private Cortex paths
are refused in all modes. Read-only
roles and review stages have no project write grant. See
[containment](../permissions-and-containment/index.md) and
[verification status](../../project/verification.md). Prompts grant no authority.
