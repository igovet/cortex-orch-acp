# Knowledge routing

Knowledge routing is model-owned guidance for selecting relevant project
reports, skills and references. It is activated only by an explicit user route;
repository state alone never activates harvest or refresh.

Bare `/harvest` can be the first request in a project session. It creates
durable, source-backed project documentation for future Cortex orchestration;
optional text specifies scope. `/harvest-refresh` updates existing documentation
against current source. Neither command requires an earlier task or report, and
neither means “summarize this chat.” Workers publish the project map under
`docs/project/` and `docs/features/`, with those directories' `index.md` files
as the entry points. Later tasks can select relevant linked pages, confirm
material claims against current source and request a refresh explicitly when
the broader inventory needs rebuilding. Completion still requires real worker
reports, independent review, runtime verification and the completion invariants.

For a refresh that changes documented behavior, defaults or output, the writer
searches existing documentation and examples for the old semantics after
writing, inventories affected executable examples, compares safe executions'
exact exit status and stdout with the documentation, and records any that cannot
be verified. A separate worker inspects current source and post-write pages
as the mandatory independent reviewer. Even a one-writer documentation task
receives review and runtime verification; passing checks do not prove every
semantic claim in a document.

Every later coordinator task receives a compact project-document catalog in
its task context: canonical project root, available index paths, source revision
and provenance. This makes the durable knowledge discoverable across sessions
without loading every page into the prompt. The coordinator follows only links
relevant to the task. A missing or ambiguous catalog is reported as such; it
cannot stand in for a successful read of the documents.

The coordinator may pass already-known source pointers in a **Known locations**
block (`path:line`, symbol and brief relevance). Workers read those sources
first and expand discovery only if the hints are insufficient or the task
affects a contract or documentation. Hints are not freshness proofs; workers
check current source. The code-index project name is pre-resolved into the prompt
when available, with `list_projects` retained as fallback. This avoids forwarding
coordinator transcripts. Structured digest-bound locations and discovery-time
metrics are deferred; prompt delivery alone does not establish a latency or
quality improvement.

Each new worker attempt also receives a fresh bounded, versioned catalog in
the actual worker engine prompt, including planned assignments, continuation
attempts and mixed-engine teams. It advises the worker to read a relevant
present index before broad source searches. An absent index leaves the worker
free to discover the project with bounded searches. Catalog titles and linked
documents are explicitly lower-trust data; delivery does not establish that
the worker followed the advice or read any page.

The catalog is bounded: it accepts at most 128 files, scans at most 4096
filesystem entries, reads at most 256 KiB per file and 2 MiB in total, and keeps
at most 128 skip diagnostics. Symlinked documentation roots and entries are
excluded. When a bound is reached, the catalog records an explicit truncation
diagnostic; it never presents an incomplete catalog as complete.

Workers use the canonical project and bounded report cursors. A missing or
stale index justifies a bounded discovery assignment; it does not authorize
private store inspection or silently broaden the task. Reports cite their
source revisions and unresolved gaps.

Documentation impact is reviewed after verified changes. The coordinator may
propose a documentation assignment when the change affects public or durable project
knowledge, then evaluates that worker's report and evidence. The runtime schedules the accepted plan and enforces review and verification.

Repository instructions in `AGENTS.md`, the current `docs/acp/*` pages and
`SECURITY.md` are authoritative for this product. The knowledge-harvest skill
governs only the harvest workflow. Historical plugin-era census and
host-qualification procedures are not part of the route.
