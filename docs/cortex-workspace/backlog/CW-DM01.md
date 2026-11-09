# CW-DM01: Workspace ubiquitous language and domain invariants

**Status:** partly built in the mock domain; written deliverables not complete · **Phase:** 1 · **Size:** M

## Goal
Workspace ubiquitous language and domain invariants.

## Requirements
CW-R17, CW-R18, CW-R19, CW-R20, CW-R21, CW-R22, CW-R23, CW-R24, CW-R25; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
Define glossary, ownership, aggregates and invariants for machines, projects, tasks, sprints, worktrees, acceptance, merge and archive. Split before implementation if effort exceeds L.

## Acceptance
- Positive: A project belongs to a machine; task execution and human acceptance remain distinct.
- Negative: A project is never treated as a machine and runtime transitions are not redefined.

## Dependencies
None.

## Validation
Write positive and negative domain or contract tests first where applicable; run focused tests and record command, environment, outcome and unrun cases.

## Documentation impact
Update the relevant planned contracts and [roadmap](../roadmap.md) as behavior and qualification change; do not describe plans as shipped.

## Status record

- Built in code: the prototype domain in `prototypes/cortex-workspace/desktop/src/workspace/domain/` enforces machine ownership of projects (a project is never a machine), optional worktrees, the separation of task execution from human acceptance, and the stop, close and archive distinctions.
- Already in [domain model](../domain-model.md): the bounded contexts and the anti-corruption rule.
- Not verified in this pass: the glossary in the domain model and the requirement wording referenced by this item in the other design documents.
