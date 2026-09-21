# Conventions

Use Python 3 and keep product behavior below `./`. Tool argument contracts
belong in the advertised MCP schemas and descriptions, not in skills or
profiles. Model proposals enter through channel-verified dispatch; runtime transitions
commit state and events transactionally.

Each project has one `.cortex/cortex.sqlite3` store. Published task documents,
reports and the current `pipeline.md` are project-local. Reports are immutable;
the pipeline is newest edition first. SQLite is the durable index and in-memory
state is only a cache.

The runtime owns orchestration, dependency scheduling, review, verification and
completion. Models submit proposals and results. Tool contracts belong in the
advertised schemas. The replacement implementation is not yet release qualified;
see `docs/acp/runtime-rewrite-status.md` for exact gaps.

Workers receive an explicit assignment, a bounded context packet and only the
reports required for that assignment. A worker may implement, verify and
document a bounded change. Add another worker only for a concrete independent
benefit.

Use `python3 -B`, preserve the no-paid default test policy, and report live ACP
client checks as unverified until the real-client runbook has a retained
observation. Release-sensitive generation, stamping, package and sync checks
run sequentially.
