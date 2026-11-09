# Domain model

**Status: planned domain model. The local desktop slice implements its identity and mapping rules for the subset below.**

A machine is a running Cortex Workspace process, not a project or device. At launch there is one local machine; remote machines connect through a server connector. Each machine owns projects; a project contains standalone tasks and optional sprints. A sprint has a separate planning orchestration that asks questions and creates linked tasks, then runs manually or sequentially in automatic mode.

At task creation, choose the current project checkout on its currently open branch, or a dedicated Git worktree. Whether concurrent tasks may write to the main checkout needs an explicit safety contract; no concurrency guarantee is established. The repository's Git default branch is the default merge target, changeable per task or sprint. Every task goes through Ready to merge, including automatic sprint execution; local merge (squash by default) or PR creation requires owner confirmation. Runtime completion and user acceptance remain distinct from integration.

DDD-lite uses a shared vocabulary and bounded contexts for machines/connections, projects/repositories, sprints, task execution/workspace, decisions/acceptance, Git/integration, archive/retention and orchestration settings. Workspace owns its domain rules; an anti-corruption mapping translates runtime facts without replacing the runtime state machine. Worktree is an optional linked Git working tree, not a project; device is a client, not a machine; archive is retained state, not purge. Do not refactor the runtime into DDD layers or introduce event-sourcing infrastructure solely for this model.

A project has a stable identity and many product tasks. Each task may have multiple executions; each execution may contain attempts, workers and verification. An ACP session is not a task identity. Decision, result, artifact and device identities are distinct and versioned.

A board phase is a projection, not a mutable runtime status. Execution state and reason are separate; disconnect shows last-confirmed time, not a guessed status. A result identifies evidence, check outcomes and workspace/artifact digest. Acceptance attaches to that digest and is invalidated or marked stale on drift. Human acceptance does not imply a commit, integration or deployment.

## Local slice identities (built)

The adapter generates public task, execution, decision and result identities. They are distinct from the runtime task identity, the ACP session identity and transcript identities; a generic ACP session identifier is never returned as a task identifier. A runtime plan revision is not a public entity version.

A description-only draft has no title. The title is null and reported as not connected; the slice does not generate one. Creating a draft creates no runtime task and no execution. Activation of Cortex for a project is a separate recorded action that does not create work. Lifecycle values map to public phases as [protocol](protocol.md) describes (mapping version 1); a value outside that mapping is shown as unknown with a warning.
