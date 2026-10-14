# Source and adaptation

This Cortex skill is a subset adaptation of the `impeccable` design skill. Its
`SKILL.md` and `references/` files are Cortex-authored text that follows the
structure and the command set of the upstream skill. Upstream prose was read
and paraphrased, not copied, except for the LICENSE and NOTICE files.

- Upstream repository: https://github.com/pbakaus/impeccable
- Exact commit: `2f98774f86bc05563876643db37e6c37c322850f` (repository HEAD at retrieval)
- Upstream files read in full: `skill/SKILL.src.md`, `skill/reference/critique.md`,
  `polish.md`, `bolder.md`, `distill.md`, `quieter.md`, `typeset.md`, `layout.md`,
  `clarify.md`.
- Upstream file read at heading level only: `audit.md`. The audit description in
  `SKILL.md` covers the categories in its headings (accessibility, performance,
  theming, responsive design, implementation integrity). A later pass that reads
  the full body should revise that description.
- Retrieved: 2026-10-14
- License: Apache-2.0, `LICENSE` at the repository root, copied byte-for-byte as
  `LICENSE` in this directory (SHA-256
  `02bb8c3b4e70190e3986c0404ad2fd8d639b4f534252d82379cc1b502b6d1812`).
- NOTICE: `NOTICE.md` at the repository root, copied byte-for-byte as `NOTICE.md`
  (SHA-256 `c60a093c2845fd9fb82f9c6f742ece31f379f8190b535309d32d66c45ccffdcb`).
  It states that the upstream iOS and Android platform references are distilled
  from `ehmo/platform-design-skills` under MIT. Those files are not included here.

## Adopted into this skill

- Command set and role split (`SKILL.md`): critique and audit for review;
  polish, bolder, quieter, distill, typeset, layout and clarify for refinement.
- `references/critique.md`: the assessment order, heuristic table, cognitive
  load checklist, severity levels, persona set and report structure, rewritten.
- `references/refinement.md`: the per-command passes, rewritten.

## Omitted

- The launcher (`skill/scripts/impeccable`), the live browser mode and its
  bundled script, the browser DOM helpers, the screenshot library, the font index
  (`scripts/data/font-index.json`), telemetry and the update and install commands.
  Reasons: the runtime and network behavior were not verified and conflict with
  Cortex's no-hidden-network stance.
- The setup, persistence, hooks, doctor, pin, init, document, extract, harden,
  onboard, animate, colorize, delight, overdrive, adapt, optimize, generate and
  live command references, which depend on the omitted tooling or on
  project-level context files this skill does not manage.
- The generated copies of the skill under agent-specific directories.
- The iOS and Android platform references (their MIT attribution is kept in
  `NOTICE.md`; they are not vendored).

## Local changes

- Package integration names role-scoped Cortex read_skill reference ids in SKILL.md,
  so supporting data can be read without target-project copies or a native file tool.

- Sub-agent instructions and a degraded-mode banner were removed; Cortex workers
  do not create subagents. The review order is recorded instead.
- Persistence, trend and ignore-file steps were removed.
- Review and refinement were split by role because the review role is read-only.
- A rule not to edit files from the review role and a "no self-scoring" rule
  were added.
