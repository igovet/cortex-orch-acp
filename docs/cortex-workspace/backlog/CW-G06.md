# CW-G06: Optional worktree lifecycle

**Status:** Planned; design gate passed 2026-11-05; implementation waits for its dependencies · **Phase:** 3+ · **Size:** L

## Goal
Optional worktree lifecycle.

## Requirements
CW-R19, CW-R25; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
Define safe checkout choice, worktree creation, task binding, cleanup and orphan handling against verified runtime behavior. Dedicated worktrees are created under the machine's worktree root, which is set on the Worktrees settings page of [CW-U07](CW-U07.md) (default `~/.local/share/cortex-workspace/worktrees`, layout `<root>/<project>/<task>`). The root is validated when it is saved and again at creation: it must be absolute, writable and outside every registered project checkout. An invalid or unavailable root blocks creation and shows the reason; it never falls back to another folder. Split before implementation if effort exceeds L.

## Acceptance
- Positive: Choose current checkout and open branch or a dedicated worktree; bind the session to the chosen directory. A dedicated worktree is created under the configured root and bound to its task.
- Negative: Unsafe concurrent writes and unconfirmed deletion are blocked; do not assume shared-checkout safety. An invalid or unavailable worktree root blocks creation and is never replaced silently.

## Dependencies
CW-F04, CW-I02, CW-D06a.

## Validation
Write positive and negative domain or contract tests first where applicable; run focused tests and record command, environment, outcome and unrun cases.

## Documentation impact
Update the relevant planned contracts and [roadmap](../roadmap.md) as behavior and qualification change; do not describe plans as shipped.
