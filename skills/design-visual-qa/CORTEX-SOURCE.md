# Source and adaptation

This Cortex skill is a distilled adaptation of the `frontend-visual-qa` skill,
with one reference file vendored. The upstream `SKILL.md` was not copied; its
evidence model and completion gate were rewritten for Cortex roles.

- Upstream repository: https://github.com/daymade/claude-code-skills
- Upstream path: `frontend-visual-qa/`
- Exact commit: `e3283248772c4842dcfa0017e6bbe38e47467a31` (repository HEAD at retrieval)
- Upstream files read: `frontend-visual-qa/SKILL.md` in part (scope and routing,
  the required outcome, the start of the workflow, and the completion gate; the
  remaining sections were read by heading only), and
  `history-derived-checklist.md` (upstream `references/` file) by heading and in
  the passages that contain tool-bound text. The file was copied verbatim, and only the passages
  named in the local changes below were edited. Its remaining text has not been
  reviewed line by line in this adaptation; a later review should do so.
- Retrieved: 2026-10-14
- License: MIT, repository-level `LICENSE`, copied byte-for-byte as `LICENSE`
  in this directory (Copyright (c) 2025 daymade; SHA-256
  `945fb5acb5c4bc9af45b54b9a02be060f5dfca5e0a77244e8aa567de63aed5d1`). The
  upstream repository has no per-skill license.

## Adopted

| Cortex path | Upstream path | Treatment | SHA-256 |
|---|---|---|---|
| `references/defect-checklist.md` | upstream `history-derived-checklist.md` | adapted (see below) | after adaptation: `3cf95270771db68950fc0f54ae74a9940d167d281bc2e1b880e221a74e549e11`; original: `61ee8f8090c8273f34c1560b9f0d2dc0dc68bd99358388caa73878433273721a` |
| `LICENSE` | `LICENSE` | verbatim | `945fb5acb5c4bc9af45b54b9a02be060f5dfca5e0a77244e8aa567de63aed5d1` |
| `SKILL.md` | `SKILL.md` | distilled and rewritten | not a copy |
| `references/banned-patterns.md` | none | Cortex-authored copy of the design-workflow DESIGN template's "Banned AI-tell patterns" section | not upstream |

## Omitted

- All scripts (`visual_layout_audit.mjs`, `silent_degradation_probe.mjs`,
  `verify_delivery.py`, `attention_inventory.mjs`, `painted_text_probe.mjs`,
  `journey_input.mjs`). They depend on the project's own Playwright installation
  and a browser, and were not executed or vendored.
- The other reference files (`journey-and-page-contracts.md`,
  `silent-degradation-and-evidence.md`, `browser-driving-and-observation-traps.md`
  and the rest of the set). They describe browser-driving procedures that the
  Cortex roles do not perform; they are not vendored.
- Tests and evaluation files.

## Local changes

- Package integration names role-scoped Cortex read_skill reference ids in SKILL.md,
  so supporting data can be read without target-project copies or a native file tool.

- `SKILL.md` was rewritten for Cortex roles: a capture role (write) and a review
  role (read-only). It keeps audit-only as the default, the evidence-level idea,
  the completion gate and the rule that an unopened screenshot has no evidence
  value. Skill references to other, unavailable skills were removed.
- The evidence levels in `SKILL.md` are a Cortex-defined simplification, not the
  upstream's exact level names.
- `references/defect-checklist.md`: the Playwright documentation link was changed
  to a general pixel-baseline note; sentences that referred to the upstream sweep
  script and its output were changed to refer to any automated or DOM inventory.
  A Cortex "Banned AI-tell defects" section was added; it points to the
  worker-readable `design-banned-patterns` reference. No other text was changed.
- The upstream section headers and standards-backed checks are otherwise kept.
