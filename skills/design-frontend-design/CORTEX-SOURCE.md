# Source and adaptation

This Cortex skill is an adaptation of Anthropic's `frontend-design` skill. It
is not a verbatim copy: the principles and the typography, color and restraint
guidance were kept, and tool-bound and persona-style text was rewritten.

- Upstream repository: https://github.com/anthropics/skills
- Upstream path: `skills/frontend-design/`
- Exact commit: `dbd4588f9e1033efb41dad4bef2f7947c8993d44` (repository HEAD at retrieval)
- Upstream `SKILL.md` blob: `a5333457c414d20d625f307df945842c0952ecc3`
- Retrieved: 2026-10-14
- License: Apache-2.0, full text in `LICENSE.txt` (copied byte-for-byte from the same
  commit). The file names no copyright holder; attribution to Anthropic follows
  the repository.
- Files adopted: `SKILL.md` (adapted text), `LICENSE.txt` (verbatim).
- Files omitted: none; the upstream skill has no scripts, references or other files.

SHA-256 of the verbatim license copy:
`0d542e0c8804e39aa7f37eb00da5a762149dc682d7829451287e11b938e94594`

## Local changes

- Package integration routes materially missing product inputs through the worker's
  advertised question tool, rather than substituting assumptions for requirements.

- Rewritten in English for the Cortex skill format with a `name`/`description`
  frontmatter that matches the directory name.
- The upstream aside about taking screenshots "if your environment supports it"
  is replaced by a rule that names the capture options available in a Cortex
  session and states what to report when none is available.
- The list of templated visual traits is kept and reworded as a list to avoid,
  with the brief-wins rule preserved.
- Added the "no self-scoring as evidence" rule and an output section.
- No instructions to spawn agents, install packages or run scripts.
