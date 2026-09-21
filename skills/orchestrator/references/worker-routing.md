# Worker routing

## Model and effort

Retain the user's selected coordinator model and effort. Do not propagate that cost
to every worker.

Every role's allowed routes -- engine, model and efforts -- are listed for you in
your own instructions, under "Allowed worker routes per role". That list is the
policy; this section is only how to choose within it. Put the chosen engine, model
and effort in the `delegate` call and repeat them in the assignment, rather than
relying on an inherited default: omitting them takes the role's first allowed
route, which is a choice you did not make.

Choose by what the work costs to get wrong, never by habit or by role name:

- Ordinary work -- fact gathering, exploration, reading sources or logs, known
  checks, documentation, implementation of already-understood steps -- takes the
  cheapest allowed route for that role, at its default effort. Do not create a
  planner to restate a small task.
- Genuinely complex or tightly coupled implementation, and ordinary code review,
  take a mid route, normally at `medium` or `high`. State the concrete complexity;
  file count alone is not it. A review route is chosen for the review, never
  inferred from the implementation's route.
- Consequential decisions -- architectural boundaries, public contracts, migration
  strategy, a plan with substantial risk -- take the strongest allowed route for
  that role. Raise the effort again for conflicting requirements, difficult
  rollback, data-loss risk or a security boundary. Such a route consumes evidence
  someone cheaper already gathered; it does not replace the gathering, and
  implementation returns to an ordinary route afterwards.
- `planner`, `architect`, `database_architect`, opt-in `senior_consultant` and
  similar follow the decision's risk, not the role name. Consultation stays
  bounded and advisory; it never changes the coordinator's own route.
- A route outside the role's allowed list is refused, and the refusal names what
  is allowed. Preserve an explicit user-requested model/effort verbatim; if it is
  not allowed for that role, say so rather than silently substituting.

Record the decision, the concrete risk or uncertainty, and the selected
engine/model/effort before assigning anything above the ordinary route. Governance
changes verification depth, not this choice. These are selection instructions and
diagnostic checks, never runtime gates.

Do not switch an active worker's model merely because it is slow or a host wait
expires.

## Assignments

The coordinator reads requirements, project instructions and bounded source evidence
needed to assess a report or decide acceptance. Delegate unknown-code discovery
to the appropriate specialist; do not investigate the same unknown code first and
then ask a worker to repeat it. An implementer may resolve bounded uncertainties
within its assignment; a separate explorer is useful only for a concrete unknown.
For structural discovery, specialists use available Codebase Memory MCP first,
following their code-and-evidence reference. Known instructions, configuration,
documentation and exact source checks need no redundant graph lookup. If the graph
is unavailable, stale or lacks relevant coverage, retain the concrete limitation
and continue with bounded native discovery; this is guidance, not a runtime gate.

Assign the complete requested deliverable, not only its research: when the user
requests a saved note or other file, include creating that file and verifying its
content/hash in the same worker assignment. The coordinator reconciles it after
the report; it does not finish omitted project-file edits itself.

Before delegation, use the project instruction's named documentation entrypoint
and follow only relevant links. A repository-wide file inventory is discovery,
not prerequisite instruction loading; leave it to the assigned specialist.
The new task context's compact project-document catalog identifies the project
root, index paths, revision and provenance. When a project has harvested
knowledge, `docs/project/index.md` and `docs/features/index.md` are its durable
entry points. Select only the relevant linked pages and give their paths to
the assigned worker; the catalog does not preload every page. Treat page claims
as source-backed leads requiring confirmation where material. Missing or stale
pages justify bounded discovery, not an automatic harvest.

The coordinator retains the causal model across roles. An investigator answers a
specific uncertainty; an implementer owns changes and self-tests; an independent
reviewer/verifier examines the result against requirements and risks. A user-visible
UI implementation is nontrivial unless the pipeline records a concrete low-risk
rationale tied to its acceptance boundary; normally use a separate reviewer or
verifier. Implementer and coordinator checks are never independent review. Combine
work only for that justified low-risk boundary. Research before implementation is
useful for unknown causes, not a mandatory stage for an already-understood edit.
Select model/effort independently for each assignment; governance changes depth,
not the model policy.

Write the assignment and every `clarify` message in English. The outgoing
engine receives its registered worker role at session creation. State English
communication, questions, reports and handoff in the brief. Do not paste the
coordinator's language rule.
Quoted source and explicitly localized deliverables retain their required language;
they do not change the worker's own communication language. After context recovery,
repeat the role/language boundary. Correct observed drift in the next native message
without restarting, blocking, translating stored evidence or hiding original text.
Each concise assignment states the role, model/effort, policy class (`research`, `exploration`, `analysis`, `ordinary`,
`complex`, `security-analysis-microtask`, `consultation`, `consultation-narrow`,
`consultation-hard`, `decision`, `decision-hard` or `review`), evidence, bounded outcome,
requirements/acceptance checks, owned files/resources/dependencies, source/report/
attachment references and handoff. Reviews retain implementation model/effort and
do not derive a route; include `Policy class: <value>`, review model/effort and
`User-requested override: yes|no` labels. Inspect complete command results.
For a verifier or reviewer dependent on a worker's result, include the exact
published predecessor `report_ref` from its `worker_result` event in the brief
and ask the worker to read it with `read_report` before checking the result.
A summary or report title does not identify that evidence.

A worker's updates, questions and blockers reach you as events on this task, and
you read them with `list_events`. Nothing about a handoff is automatic, for you or
for a worker: an assignment ends when its worker publishes a report and calls
`publish_result`, and you learn of it as a `worker_result` event carrying that
report's id.

The route requested in `delegate` or repeated in its brief is a plan, not execution
evidence. Before a pipeline or final response names a worker engine, model or effort,
read the current capsule and copy that assignment's exact `observed_worker_route`.
Copy assignment, report, artifact and check identifiers from current durable
receipts. Omit an unavailable value instead of reconstructing it from prose or a
planned route.

You do not wait for a worker yourself and you cannot poll one. The runtime holds
the turn while workers run and brings you their results as they arrive; a turn you
end while work is still running simply leaves that work running. Answer the user
when the task is decided, not when you have run out of things to say.

If the worker's Cortex MCP channel is unavailable, keep the assignment
incomplete and report the concrete failure. Do not substitute an engine-native
subagent result for the required report and `publish_result` receipt.

Include only the bounded assignment and needed references, not pasted protocols,
schemas or checklists. Workers receive the role instructions at engine-session
creation and can read situational skills through `read_skill`.

Use these 23 profiles:

| Profile | Select for |
| --- | --- |
| `accessibility_auditor` | WCAG and assistive-tech audit |
| `accessibility_fixer` | Accepted accessibility remediation |
| `architect` | Consequential contracts and boundaries |
| `backend_dev` | APIs, logic, and persistence |
| `build_verification` | Independent build/release evidence |
| `code_reviewer` | Defect-focused change review |
| `data_engineer` | ETL, migration, and integrity work |
| `database_architect` | Schema, index, and query-plan design |
| `debugger` | Reproduction, root cause, and repair |
| `devops_engineer` | CI/CD and runtime configuration |
| `explorer` | Unknown paths, ownership, and impact |
| `frontend_dev` | Browser UI and tests |
| `fullstack_dev` | Cohesive client/server change |
| `general` | No justified narrower specialty |
| `mobile_dev` | iOS, Android, React Native, Flutter |
| `performance_engineer` | Measurement and optimization risk |
| `planner` | Execution-changing work breakdown |
| `qa_engineer` | Coverage, regression, and quality evidence |
| `refactorer` | Behavior-preserving restructuring |
| `security_auditor` | Auth, secrets, crypto, dependency risk |
| `senior_consultant` | One decision from selected reports |
| `technical_writer` | Source-backed documentation |
| `ux_designer` | Flows, hierarchy, responsive interaction |
