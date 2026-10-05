---
name: knowledge-harvest-scope
description: Scope explicit harvest and harvest-refresh requests from retained evidence, without coordinator repository census.
---

# Harvest scope assessment

Only explicit /harvest or /harvest-refresh authorizes this work; optional text
narrows scope and no previous task is required. Ordinary product work does not
authorize a census or broad documentation rewrite. Harvest builds a complete
source-backed map; refresh rebuilds the inventory against current source,
corrects stale knowledge and finishes with a post-write comparison.

For harvests only, repository reads, source analysis, edits, commands and
completeness checks are worker-owned. Read report previews, the pipeline and
bounded evidence pages needed for a scope decision, not project indexes or
runtime implementation. Missing knowledge indexes justify bounded census work,
not coordinator source inspection. This restriction does not apply to ordinary
orientation, direct answers or evidence reconciliation.
Require harvest workers to read `knowledge-harvest` before census work; it owns
the canonical output layout, complete inventory and coverage contract. Workers
confirm documentation in current source and report unresolved gaps, not guessed
coverage percentages.

## Scale the assignment to the repository

Choose the fewest assignments that can establish complete, source-backed
coverage. A small repository with a bounded source tree, one runtime, and few
feature-bearing surfaces can use one `technical-writer` worker for the whole
harvest or refresh. Assign that worker to enumerate the current tree, inspect
source, tests and configuration, map every in-scope surface, write or update the
canonical documents, run relevant deterministic checks, and report exact gaps
and outcomes. This is one cohesive assignment, not a sequence of planner,
explorer, architect, writer, reviewer and verifier handoffs. Its self-checks
are evidence of what ran, not independent review.

For a broad or coupled repository, partition only when the source tree or
ownership makes one assignment impractical. Delegate bounded `explorer`
domains where independent discovery reduces uncertainty; use `planner` for a
real scope or dependency problem and `architect` for a consequential boundary
decision. Give exactly one writer ownership of shared `docs/project/` and
`docs/features/index.md`; other writers may own disjoint feature pages. A
worker needing earlier evidence receives only the relevant finalized reports.

After writing, compare the current source inventory with the coverage matrix,
feature pages, links and checks. For each correction to a documented behavior,
default, or output (including one found against unchanged source), inventory
every relevant occurrence across the target repository's documentation,
including otherwise unchanged pages, commands, flags, example inputs and
expected output. Confirm each claim in current source and correct stale examples.
Inventory every affected executable example across those pages;
run safe examples against the current source and compare exact exit status and
stdout with their documented expectations. Mark examples that cannot safely
run or whose output cannot be verified as unresolved, naming the reason.
The writing worker may perform this comparison and repair bounded gaps. The runtime supplies an independent reviewer for these changed documentation claims. The reviewer checks
current source and post-write documentation, including the complete relevant example inventory
and observed results, without relying on the writer's claim that a search was
clean. Propose additional specialist investigation for a concrete unresolved risk, such as ambiguous feature boundaries, conflicting source evidence, unexplained
surfaces, consequential safety behavior, or checks the writer cannot run or
interpret. Do not repeat a check on an unchanged revision without a reason. A small refresh
may use one writing assignment; independent review and command verification remain
mandatory runtime stages. Report unresolved gaps honestly. Models propose
adaptations; the runtime owns scheduling, review and completion.

## Relationship to the final documentation stage

A harvest or harvest-refresh is documentation-impacting by definition. Its
documentation delegations serve as the dedicated documentation-sync worker or
workers for `docs/project/`, `docs/features/`, and any affected public
documentation. After the final writing or corrective delegation, the
coordinator assesses the worker's source comparison and checks and submits a
synthesis grounded in retained evidence. The runtime requires independent review
and automatic verification before completion. It may follow bounded report cursors for facts needed by
that decision, while documentation indexes and project files remain worker-owned.
Missing specialist interpretation requires a bounded worker check.

Do not use a task-level `documentation not required` outcome for a completed
harvest, because documentation is the requested artifact. Public documents
that the worker reports as unaffected should remain unchanged rather than
receiving meaningless edits.

