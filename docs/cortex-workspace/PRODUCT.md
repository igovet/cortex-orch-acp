# Cortex Workspace product

**Status: the Ubuntu desktop product (tasks, sprints, decisions, results, Git, files, terminal, workers and local settings) is implemented and checked by no-paid automated checks; it is not verified on a real Ubuntu session, with paid model runs or as a published release. Remote machines, relay, push and Android are planned and not implemented. Each requirement and backlog item states its own status ([requirements](requirements.md), [backlog](backlog/index.md)).**

A task-first workspace for working with agents: one project contains user tasks; each task contains discussion, executions, decisions and versioned results. Board cards are tasks, not workers. Desktop emphasizes board/list and a task workspace; Android emphasizes decisions and changes since last visit. A small request should not require a complex workflow.

The first release is the Ubuntu desktop, which runs the execution machine and the client, with built-in code and Git workflows, including push, and no separate user-installed software. Planned and not implemented: Android and remote access (remote machines are in M5 and the Android client is in M6, after Gate M), with relay and push services on an owner-operated server so that clients install nothing extra. Desktop UI concepts precede mobile concepts and implementation. The runtime, not the client, owns execution, review and verification. Acceptance by a person is separate from runtime completion, commit, push and release. Generic ACP support must expose only capabilities actually negotiated.

See [requirements](requirements.md), [screens](SCREENS.md) and [roadmap](roadmap.md).
