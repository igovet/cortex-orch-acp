# Source and adaptation

This Cortex skill adapts the workflow guidance in Anthropic's `skill-creator` skill. It is not a verbatim copy of that skill.

- Requested source: [anthropics/skills skill-creator](https://github.com/anthropics/skills/tree/main/skills/skill-creator)
- Resolved revision: `33375500bcea98d610eb30ce10ac4e59b89c390d` (the repository's `main` head resolved on 2026-09-27).
- Exact source file: [SKILL.md at the resolved commit](https://github.com/anthropics/skills/blob/33375500bcea98d610eb30ce10ac4e59b89c390d/skills/skill-creator/SKILL.md)
- Retrieved and checked: 2026-09-27.
- Upstream license: Apache-2.0; see [LICENSE.txt at the same commit](https://github.com/anthropics/skills/blob/33375500bcea98d610eb30ce10ac4e59b89c390d/skills/skill-creator/LICENSE.txt). Copyright Anthropic, PBC.
- Inspected: upstream `SKILL.md`, `references/schemas.md`, and `LICENSE.txt`. No upstream source files, scripts, templates, schemas, or assets are copied into this Cortex skill.
- Adapted ideas: capture user intent and examples, make skill metadata explain its trigger, keep supporting material separate, test with representative prompts, and improve from reviewable feedback.
- Cortex-specific changes: runtime-owned planning and permissions, coordinator-only guidance, exact-revision provenance for imports, untrusted-source handling, the guarded relative Claude link, no automatic skill creation, and no assumed subagents, paid evals, browser, or upstream helper scripts.

The reference URL for this skill is deliberately pinned above; later upstream changes do not silently alter this adaptation.
