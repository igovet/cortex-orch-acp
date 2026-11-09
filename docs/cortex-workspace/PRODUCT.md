# Cortex Workspace product

**Status: planned; no implementation or qualification is implied.**

A task-first workspace for working with agents: one project contains user tasks; each task contains discussion, executions, decisions and versioned results. Board cards are tasks, not workers. Desktop emphasizes board/list and a task workspace; Android emphasizes decisions and changes since last visit. A small request should not require a complex workflow.

The first release is the Ubuntu desktop, which runs the execution machine and the client, with built-in code and Git workflows, including push, and no separate user-installed software. Android and remote access follow later: remote machines are in M5 and the Android client is in M6, after Gate M. Relay and push services run on an owner-operated server, and clients install nothing extra. Desktop UI concepts precede mobile concepts and implementation. The runtime, not the client, owns execution, review and verification. Acceptance by a person is separate from runtime completion, commit, push and release. Generic ACP support must expose only capabilities actually negotiated.

See [requirements](requirements.md), [screens](SCREENS.md) and [roadmap](roadmap.md).
