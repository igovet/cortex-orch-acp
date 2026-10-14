# Source and adaptation

This Cortex skill adapts a reduced subset of the `ui-ux-pro-max` skill. The
upstream project bundles a search engine, a large data set and an installer;
none of the executable parts are included here.

- Upstream repository: https://github.com/nextlevelbuilder/ui-ux-pro-max-skill
- Upstream path: `.claude/skills/ui-ux-pro-max/`
- Exact commit: `7246805992a42b7bba37d4cac83b01416f0c6c44` (repository HEAD at retrieval)
- Retrieved: 2026-10-14
- License: MIT, `LICENSE` at the repository root, copied byte-for-byte as
  `LICENSE` in this directory. Copyright (c) 2024 Next Level Builder.

## Adopted files

| Cortex path | Upstream path | Treatment | SHA-256 |
|---|---|---|---|
| `LICENSE` | `LICENSE` | verbatim | `738f69dfa83db5c347c678fb9d90e560877059f0de93a327c39001bff92dc014` |
| `references/colors.csv` | `data/colors.csv` | verbatim | `8162429222bce22df62b564085946a30d07cc9722c58d0a3a494bd0d1d00841c` |
| `references/typography.csv` | `data/typography.csv` | verbatim | `321fc446e89024488ebae96dda93efc4d2307bd8bddb240857ad51364f6782c8` |
| `references/ux-guidelines.csv` | `data/ux-guidelines.csv` | verbatim | `ff81ec613f70ba9fc3fcce52dbe4ae35d44b2079dbe6dc066d2d6e38c28facd5` |
| `references/pro-rules.md` | `references/pro-rules.md` | adapted (four lines, see local changes) | upstream `d28442d61c310b49c054a4ea736f89fb3008beebdbbc38c4b3a540b8f7fadf23`; adapted file `2354a332510e77aabc5d01e13ee17e377ed467d6af32786e04fefe65eb0e1d5a` |
| `SKILL.md` | `SKILL.md` | rewritten (see below) | not a copy |

## Omitted

- All scripts (`scripts/`, including the search, design-system and reasoning
  code) and their tests. They were not executed or vendored.
- The data files `styles.csv`, `products.csv`, `ui-reasoning.csv`, `landing.csv`,
  `charts.csv`, `app-interface.csv`, `motion.csv`, `icons.csv`, the stack tables,
  `google-fonts.csv`, `phosphor-icons-upstream.json`, `google-font-licenses.json`
  and `data-provenance.json`. They are bulk data and would not be reachable
  without the omitted search tooling.
- The upstream `quick-reference.md` (rule text used by the search tool). Links to
  it were removed from the adapted `pro-rules.md`.
- The CLI, gallery, screenshot and CI directories.

## Local changes

- Materially missing product inputs use the worker's advertised question route;
  non-material implementation assumptions remain labeled.

- Package integration names role-scoped Cortex read_skill reference ids in SKILL.md,
  so supporting data can be read without target-project copies or a native file tool.

- `SKILL.md` was rewritten: the search-tool workflow and command syntax were
  removed; the skill now points to the adopted reference files directly.
- Scope limited to product UI foundations, with a note that backend and
  logic-only work is out of scope.
- Added the "no self-scoring as evidence" rule and the output requirements.
- The Google Fonts URLs and CSS snippets inside `typography.csv` are kept as
  data; the skill states they are not fetched.
- `references/pro-rules.md`: four lines changed. The opening paragraph and the
  scope notice pointed to a priority table and to the omitted `quick-reference.md`;
  they now point to `ux-guidelines.csv`. Two checklist items in the Process section
  referred to the omitted file and to a search command; they now refer to
  `ux-guidelines.csv`. No other text was changed.
