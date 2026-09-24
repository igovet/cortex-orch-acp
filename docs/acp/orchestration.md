# ACP orchestration

Cortex is the incoming ACP agent and the owner of orchestration. ACP transports
messages and sessions; the models propose plans, solve assignments and synthesize
results. The runtime chooses what executes and whether the task can complete.
See [implementation status](runtime-rewrite-status.md) for retained test evidence
and the separate real-client qualification boundary.

`orchestration/state.py` stores task state, original goal, plan revision, dependency
nodes, structured results, events and request receipts in `.cortex/cortex.sqlite3`.
State changes and their events commit together. A plan is validated for exact fields,
known roles, unique node names, existing dependencies, cycles, bounded scope and
verification commands. Each new plan node requires a `model` recommendation (`auto`, engine id, or `engine/model`) and may include `model_reason`; neither field grants dispatch authority. The runtime computes tier and effort and revalidates policy and availability. See [automatic routing](routing-and-fallback.md#automatic-worker-routing-tiers-model-hints-and-ranking).

`orchestration/controller.py` drives planning, execution, independent review,
verification and synthesis. Only dependency-ready nodes are reserved for execution.
Reservations commit before provisioning, and concurrent claims share the same
four-worker ceiling. Runtime routing resolves model and effort from the current tier, policy, catalog and audit balance; plan hints remain advisory. All
23 specialist/consultant profiles remain available.

The runtime inserts an independent code reviewer after execution whenever the
planned work changed the project. When a plan revision starts executing, the runtime
records an `EXECUTION_BASELINE` observation of the project (the same read-only tree
digest and HEAD used for review and verification binding). When every node has
completed, review is skipped only if the project observed now equals the first
baseline since the task was created or last reopened, or a state an independent
reviewer approved. The runtime then records `REVIEW_NOT_REQUIRED` with the reason
and digest, projects it into the pipeline's "Independent review" section, and goes
straight to verification, which binds to that digest. Any other observation, an
unavailable observation, or a revision that already has a reviewer (a fix cycle)
schedules review. Retried nodes and fix nodes are covered because the decision
compares the whole project, not individual worker claims. No model argument or
hint can waive review; `EXECUTING → VERIFYING` is refused without the runtime's own
record. APPROVE proceeds
to verification; REQUEST_CHANGES creates a fix node followed by a fresh reviewer;
BLOCK stops acceptance. Three unsuccessful review cycles block further automatic
work. The loop never treats an ordinary final model message as a worker result.

Worker results bind to a published report owned by the exact provisioned attempt.
The result receipt retains a content digest. A passed check may cite a completed,
zero-exit `command_run` or user-confirmed `outside_command` receipt from that
same task and attempt; a cited failed check requires a completed nonzero exit.
Pending, duplicated, foreign or mismatched receipts cannot establish an outcome.
Every synthesis `not_verified` item requires a nonblank concrete blocker.
Command verification runs literal argv
in an explicit Linux namespace sandbox with no user home, inherited credentials,
network, or access to the real runtime store. Missing isolation, failed commands,
incomplete output or a changed project digest cannot establish a pass.

Completion of planned work requires current completed nodes, an approving review
(or the runtime's `REVIEW_NOT_REQUIRED` record for an unchanged project), complete passing
verification for the exact plan/project revision, intact reports, synthesis and no
live or unknown worker session. Runtime pipeline editions project the durable state;
the pipeline is one newest-first Markdown file and other reports remain immutable.

For a simple informational question, the coordinator reads relevant files and
submits an answer without a plan or workers. The runtime transitions through
ANSWER_READY and validates the exact answer receipt before completion. Any plan
revision, node or worker provisioning disallows this path. Project mutations stay
unavailable to the coordinator; a direct answer never reaches the review stage,
and planned work that changes the project retains the full review gate.

Paseo hides thinking and protocol bookkeeping (including wait) in chat and worker
panels, live and on replay. Meaningful MCP operations, private-store tool cards,
CUA calls and generic/titleless tool updates retain readable, expandable details. Logical tool IDs remain stable across lifecycle updates and replay.
Credential redaction remains; a downstream terminal reference cannot be relayed as
if it belonged to the incoming client's terminal namespace. Only actually exposed
engine events can be forwarded—absent native events are never fabricated.

Engine processes run in the engine jail, where their native tools write only the
assignment scope; native subagents are disabled. The runtime gateway
checks current channel authority and assignment scope before each operation,
contains commands and external MCP servers with bubblewrap, and retains exact
receipts. See [permissions and containment](permissions-and-containment.md).

Steering fences existing workers before replanning. Worker questions persist an
exact pending question and pause execution; only a real matching answer resumes
it. Every answered question, with its question text, the user's verbatim answer,
ids, timestamps and answering session, is visible to later workers and the
reviewer through `list_sources`/`read_source` and the recovery capsule's
constraints (see [input history](input-history-and-replay.md#answered-questions)).
Lost workers are retried at most twice after confirmed absence, and uncertain
action receipts prevent replay. A worker whose route itself failed (an
engine-reported exhausted plan, rate limit or unavailable model, or an engine
that would not start or confirm the worker's mode) is instead retried on the
next available route, possibly another provider. Those retries don't count
against the two ordinary retries and are bounded separately. A routing refusal
stops the work under its own name and is never retried. See
[routing and fallback](routing-and-fallback.md#automatic-worker-routing-unavailable-routes). Coordinator, worker and review turns have total
and idle deadlines. Scope reservations remain occupied until engine termination
is observed. A task waiting for another task's capacity remains pending with a
bounded wait. Senior escalation inserts a read-only consultant routed automatically
by the runtime. Three accepted automatic replans and three review cycles bound the
respective loops; exhausted limits require user input or a new plan.
