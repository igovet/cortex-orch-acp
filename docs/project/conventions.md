# Conventions

Use Python 3 and keep product behavior below `./`. Tool argument contracts
belong in the advertised MCP schemas and descriptions, not in skills or
profiles. Mutations enter the store through the channel-verified dispatch
path.

Each project has one `.cortex/cortex.sqlite3` store. Published task documents,
reports and the current `pipeline.md` are project-local. Reports are immutable;
the pipeline is newest edition first. SQLite is the durable index and in-memory
state is only a cache.

The model owns coordination and semantic acceptance. The runtime owns storage,
identity, isolation, idempotency, provenance and recovery. No mandatory
specialist sequence, hidden approval machine or compatibility route may be
introduced.

Workers receive an explicit assignment, a bounded context packet and only the
reports required for that assignment. A worker may implement, verify and
document a bounded change. Add another worker only for a concrete independent
benefit.

Use `python3 -B`, preserve the no-paid default test policy, and report live ACP
client checks as unverified until the real-client runbook has a retained
observation. Release-sensitive generation, stamping, package and sync checks
run sequentially.
