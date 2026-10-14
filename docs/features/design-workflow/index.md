# UI/UX design workflow

Source: `skills/design-workflow/` (`SKILL.md` and `references/`), the five design
specialist sources under `agent-sources/` (`ux-designer`, `frontend-dev`, `mobile-dev`,
`qa-engineer`, `accessibility-auditor`), the `design-*` skill directories under
`skills/`, and the registration in `src/cortex_runtime/portable/roles.py`.

The design workflow is coordinator guidance that arranges existing roles. It adds no
agent, no new profile, no MCP tool, no design store and no acceptance gate. The runtime
still owns scheduling, permissions, independent code review, verification and
completion. Intake that precedes design is described in
[requirements intake](../requirements-intake/index.md).

## Activation

The coordinator reads this skill for new UI/UX design, a visual redesign, a design
system, or a significant visual improvement. It does not apply to backend work, to bug
fixes without visual change, to routine frontend coding, or to functional UI changes that
need no design decisions. A routine change normally needs one frontend-dev, not the
full sequence below. The orchestrator skill carries this routing
(`skills/orchestrator/SKILL.md`); the runtime does not detect activation.

## Design intake and project documents

Before design work the coordinator reads the target project's instructions and, when
present, `docs/PRODUCT.md`, `docs/DESIGN.md` and `docs/SCREENS.md`, or equivalent existing
sources and established tokens and components. Existing approved design systems are
respected, not replaced blindly.

Only material design questions are asked, using the
[design intake bank](../../../skills/design-workflow/references/design-intake.md):
product and primary job, audience, platforms, screens and content, direction, references,
avoid list, constraints and risks, and acceptance and verification. At most five
questions per round, normally one round and at most two. Missing files alone do not
justify questions when the request and code already answer the points.

When documents are missing or insufficient, a technical writer records the
agreed answers before design work, preserving existing facts and naming unresolved
choices. Cortex does not create the target project's files by itself; the writer follows
these outlines:

| Target file | Outline (bundled template) | Records |
| --- | --- | --- |
| `docs/PRODUCT.md` | [PRODUCT](../../../skills/design-workflow/references/PRODUCT.md) | Outcome and audience, scope, platforms and constraints, success criteria, decisions and unresolved items |
| `docs/DESIGN.md` | [DESIGN](../../../skills/design-workflow/references/DESIGN.md) | Product basis, art direction, foundations (type, palette, spacing, radius, asset licensing), components and interactions, responsive behavior, avoid and preserve list, visual acceptance |
| `docs/SCREENS.md` | [SCREENS](../../../skills/design-workflow/references/SCREENS.md) | One section per in-scope screen: purpose, required content, navigation, actions, states, long content, size classes, keyboard and semantics, acceptance |

Coordinator-readable reference ids are `design-intake`, `PRODUCT`, `DESIGN`, `SCREENS` and
`visual-review`, resolved through `read_skill`; the target project does not need these
package files. Design is never invented from an aesthetic reference: a reference informs
principles, not product features.

## Phases and role ownership

The steps scale to design risk. Approved direction is reused, and research or documentation
steps that are already sufficient are omitted. For substantial visual change, evidence and
independent review are explicit in the plan objectives and dependencies.

| Step | Owner and access | Output |
| --- | --- | --- |
| 1. Art direction | `ux-designer`, read-only | Report with principles, flows, type, color and spacing tokens, component states, responsive rules and rationale. It does not write `DESIGN.md`. |
| 2. Persist | Technical writer or assigned writing role | Chosen direction recorded in `docs/DESIGN.md`, with `PRODUCT.md` and `SCREENS.md` aligned. Implementation follows that spec. |
| 3. Implement and capture | `frontend-dev` or `mobile-dev` (write); `qa-engineer` may capture evidence | Code, plus captures at mobile, tablet and desktop sizes (or a stated platform-specific size class), with exact dimensions. One active writer owns each screen or shared UI file set. |
| 4. Independent review | `ux-designer` and `accessibility-auditor`, read-only, in a fresh, separate review | Concrete findings against `DESIGN.md` and `SCREENS.md`. Reviewers open the writer's captures and create no files. |
| 5. Refine | The same implementation owner | Fixes, then recaptured and rechecked affected states. At most two planned refinement rounds; material findings still open afterwards go to a scope decision or replan. |

Runtime code review is separate and mandatory; visual review supplements it and never
replaces it. Read-only roles are listed in `orchestration.contracts.READ_ONLY`
(`ux-designer` and `accessibility-auditor` among them).

## Visual quality gate and evidence

The coordinator applies the [visual review checklist](../../../skills/design-workflow/references/visual-review.md).
It covers art direction against `DESIGN.md`; typography, readability, hierarchy and line
length; composition, alignment and spacing rhythm; reflow, clipping, overlap and long or
localized content; required content and every in-scope state against `SCREENS.md`;
navigation, feedback and recovery; contrast, semantic controls, targets, keyboard focus and
reduced motion; and originality against the approved brief.

Evidence rules:

- Evidence binds to the implemented revision, screen or route, actor or data state, and
  viewport. Mobile, tablet and desktop captures are opened and inspected, including lower
  page sections. A justified native-platform exclusion is stated with its reason.
- Screenshots are stored in a git-ignored project path outside private storage. The
  example path in the skill, `.design-evidence/`, is ignored by this checkout's
  `.gitignore` (`/.design-evidence/`); a target project that uses it must add the same
  rule first and verify it (for example with `git check-ignore`), or choose another
  ignored path.
- Model self-scores, passing builds, unit tests and the presence of DOM nodes are not
  evidence of visual quality. Unopened screenshots are not evidence either.
- Screenshots alone do not prove interactions or accessibility conformance. Keyboard,
  focus and, where available, assistive-technology checks are recorded separately.
- Findings carry severity, user impact, region, viewport, state and reproduction. Recheck
  covers the same states after fixes.
- Missing evidence and unresolved material findings remain explicit blockers in the
  reported outcome. They are not turned into diagnostic passes.

## Bundled design skills

The five skills are shared Cortex package content under `skills/design-*/`. They are
worker-only; coordinator channels cannot read them. Each is readable only by the roles in
`DESIGN_SKILL_ROLES` (`src/cortex_runtime/portable/roles.py`), and backend workers and the
senior consultant do not receive them. The runtime does not gate their use; they are read
on demand through `read_skill` when their trigger arises.

| Skill | Stage | Readable by | Provenance |
| --- | --- | --- | --- |
| [`design-reference-analysis`](../../../skills/design-reference-analysis/CORTEX-SOURCE.md) | Reference research | `worker-ux-designer` | Cortex-authored. Informed by an upstream design-reference skill whose license could not be established, so no upstream text is vendored. |
| [`design-ui-ux-pro-max`](../../../skills/design-ui-ux-pro-max/CORTEX-SOURCE.md) | Design system | `worker-ux-designer`, `worker-mobile-dev` | Reduced subset of `nextlevelbuilder/ui-ux-pro-max-skill` (MIT, pinned `7246805`). Color, typography and UX-guideline tables are verbatim; no scripts. License in `LICENSE`. |
| [`design-frontend-design`](../../../skills/design-frontend-design/CORTEX-SOURCE.md) | Implementation | `worker-frontend-dev`, `worker-mobile-dev` | Adapted from `anthropics/skills` `frontend-design` (Apache-2.0, pinned `dbd4588`). License in `LICENSE.txt`. |
| [`design-impeccable`](../../../skills/design-impeccable/CORTEX-SOURCE.md) | Critique and refinement | `worker-ux-designer`, `worker-accessibility-auditor` (review, read-only); `worker-frontend-dev` (edits) | Subset of `pbakaus/impeccable` (Apache-2.0, pinned `2f98774`). `LICENSE` and `NOTICE.md` ship verbatim. Launcher, live mode and platform references are omitted. |
| [`design-visual-qa`](../../../skills/design-visual-qa/CORTEX-SOURCE.md) | Capture and rendered review | `worker-ux-designer`, `worker-accessibility-auditor`, `worker-frontend-dev`, `worker-mobile-dev`, `worker-qa-engineer` | Distilled from `daymade/claude-code-skills` `frontend-visual-qa` (MIT, pinned `e328324`). The checklist is adapted; scripts are omitted. License in `LICENSE`. |

Taste Skill (`senlindesign/taste-skill`) is **not** bundled. The 2026-10-14 audit found no
license file in that repository, so its text cannot be vendored. The Cortex-authored
`design-reference-analysis` provides an equivalent method, not its text.

The design-* skill directories contain no executable scripts, launchers or installers, and
the adapted copies omit the upstream network and browser-install instructions. Each
`CORTEX-SOURCE.md` records the exact upstream commit, adopted and omitted files, and local
changes.

Design reference data (for example the color and typography tables, the Impeccable
critique and refinement passes, and the visual defect checklist) is registered as
role-scoped package references under the same allowlist as its parent skill.

## Project extra skills

Projects add their own procedures with `/skill-management`, described in
[commands and project skill management](../skill-management/index.md). A project skill
is created under `.agents/skills/<name>/` and listed to the session; it is read by its
exact path through `read_skill`.

The design workflow asks workers to prefer an applicable session-listed project skill over
a bundled one, subject to project instructions, authorization and the agreed brief. That
is prompt guidance; the runtime does not rank skills.

## Browser and Playwright status

Cortex has no native browser tool. A browser is available to a worker only when an MCP
server that provides one is actually advertised in its session and usable. Playwright MCP
is an external, owner-configured server. Configuration is not proof of availability.
Without a usable browser, the workflow uses the project's supported screenshot tooling or
user-supplied captures. If neither exists, rendered claims are reported as unverified and
the missing capability is named. The workflow never installs a browser, repairs a registry,
adds telemetry, or assumes that a server exists.

Status as established on 2026-10-14: in the session that was diagnosed, the owner-configured
`playwright` server closed immediately (`CONNECTION_CLOSED`). The diagnosis found no
repository defect:

- The owner registry (`$XDG_CONFIG_HOME/cortex/mcp-servers.json`, `servers.playwright`) named
  a Node executable path that does not exist on the host. Its `read_paths` listed the same
  missing directory.
- The current checkout's `mcp_bridge.host_mcp_servers()` drops a server whose grant paths are
  missing (`missing_grant_paths`) and logs `external_mcp_server_path_missing`. The deployed
  package used for that session predates this filter, so it forwarded the entry; the proxy
  then refused to start the child, which the client reported as a closed connection.
- A manual probe with the registry's arguments and a real Node executable completed the MCP
  `initialize` and `tools/list` exchange, so the server itself works once its paths are valid.

**Owner actions** (configuration outside this repository; not performed by Cortex):

1. Make the `playwright` entry's `command` and its matching `read_paths` entry point at a
   Node executable that exists. Either edit those two values by hand, keeping the existing
   `args`, or re-run `python3 -B scripts/cortex_install_playwright_mcp.py` from the checkout.
   The installer pins `@playwright/mcp` 0.0.81, probes the server, and needs an existing
   pinned Playwright Chromium or Google Chrome; it does not download a browser. It
   **replaces** `args` with its defaults (headless, isolated, Chromium), so a custom
   configuration such as the `--extension` bridge mode is lost.
2. Refresh the deployed Cortex package so it includes the missing-path filter. Do this while
   no session is running.

Until both are done, browser evidence for the design workflow remains unverified.

## Limits

- The workflow is prompt-level guidance. Coordinator activation, question bounds and
  reviewer behavior are model behavior; the runtime's controls are the plan, the mandatory
  independent review and verification.
- Registration, access by role, package content and the presence of the design rules are
  covered by `tests/test_portable_roles.py` and `tests/test_prompt_drift.py`. Rendered
  design quality is not covered by any automated test in this repository.
- Upstream licenses and provenance were checked at the commits named above; later upstream
  changes are not tracked.

See also [roles and delegation](../roles-and-delegation/index.md#instruction-delivery),
[user input, questions and cancellation](../steering-and-questions/index.md) and
[verification](../../project/verification.md).
