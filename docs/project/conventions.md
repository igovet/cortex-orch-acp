# Conventions

The product is Rust and contains no Python; keep product behavior below `./`. Tool argument contracts
belong in the advertised MCP schemas and descriptions, not in skills or
profiles. Model proposals enter through channel-verified dispatch; runtime transitions
commit state and events transactionally.

Each logical project has one [private home store](storage.md#where-the-database-lives-and-how-it-moves) per channel. Linked worktrees share their channel store. The SQLite index, published documents, reports and current `pipeline.md` live together. Reports are immutable;
the pipeline is newest edition first. SQLite is the durable index and in-memory
state is only a cache.

The runtime owns orchestration, dependency scheduling, review, verification and
completion. Models submit proposals and results. Worker questions and additional
path access requests park only their assignment; a real user answer or validated
access decision continues in the same healthy session. Tool contracts belong in
the advertised schemas. Release readiness is decided by the checks in [verification](verification.md).

Workers receive an explicit assignment, a bounded context packet and only the
reports required for that assignment. A worker may implement, verify and
document a bounded change. Add another worker only for a concrete independent
benefit.

Use the Rust tooling (`scripts/cortex-xtask.sh`, `scripts/cortex-paseo-tools.sh`), preserve the no-paid default test policy, and report live ACP
client checks as unverified until observed and retained; follow the isolated
dev Paseo procedure in `docs/project/dev-environment.md`. Release-sensitive generation, stamping, package and sync checks
run sequentially.
