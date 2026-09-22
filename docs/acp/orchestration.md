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
verification commands. A model cannot include engine, model, effort or permissions
in the plan schema.

`orchestration/controller.py` drives planning, execution, independent review,
verification and synthesis. Only dependency-ready nodes are reserved for execution.
Reservations commit before provisioning, and concurrent claims share the same
four-worker ceiling. Existing role configuration resolves model and effort. All
23 specialist/consultant profiles remain available.

The runtime inserts an independent code reviewer after execution. APPROVE proceeds
to verification; REQUEST_CHANGES creates a fix node followed by a fresh reviewer;
BLOCK stops acceptance. Three unsuccessful review cycles block further automatic
work. The loop never treats an ordinary final model message as a worker result.

Worker results bind to a published report owned by the exact provisioned attempt.
The result receipt retains a content digest. Command verification runs literal argv
in an explicit Linux namespace sandbox with no user home, inherited credentials,
network, or access to the real runtime store. Missing isolation, failed commands,
incomplete output or a changed project digest cannot establish a pass.

Completion of planned work requires current completed nodes, an approving review, complete passing
verification for the exact plan/project revision, intact reports, synthesis and no
live or unknown worker session. Runtime pipeline editions project the durable state;
the pipeline is one newest-first Markdown file and other reports remain immutable.

For a simple informational question, the coordinator reads relevant files and
submits an answer without a plan or workers. The runtime transitions through
ANSWER_READY and validates the exact answer receipt before completion. Any plan
revision, node or worker provisioning disallows this path. Project mutations stay
unavailable to the coordinator; planned work retains the full review gate.

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
it. Lost workers are retried at most twice after confirmed absence, and uncertain
action receipts prevent replay. Coordinator, worker and review turns have total
and idle deadlines. Scope reservations remain occupied until engine termination
is observed. A task waiting for another task's capacity remains pending with a
bounded wait. Senior escalation inserts a read-only consultant routed automatically
by the runtime. Three accepted automatic replans and three review cycles bound the
respective loops; exhausted limits require user input or a new plan.
