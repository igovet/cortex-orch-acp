# Adaptive worker profiles

Cortex builds worker instructions from a common operating profile, the role's
specialization, shared report and verification guidance, and any competencies
selected for the work. The native launch also receives the assignment brief,
current recovery capsule, project instructions and the channel's actual tool
catalogue. The execution brief can include bounded fact memory from the same
task or project, with provenance, where the source falls within the execution
scope and the current checkout's file digest matches. Workers confirm relevant
observations in current sources and open retained reports through their existing
authorized report routes.

## Profiles, compatibility and permissions

Five sources in [agent-sources/base](../../agent-sources/base) describe
investigation, implementation, independent review, verification and consultation.
The [profile catalogue](../../profiles.json) renders five actual templates.
Fresh plans choose Explorer for read-only investigation or General for
implementation, with domain competencies as needed. Review, verification and
consultation are reserved to the runtime. Retained node roles resolve to one of
the five templates. The stored role continues to determine tool permissions,
route settings, report templates and audit labels for that node. New plans do not
inherit a pin for a different historical role.

| Role | Base | Specialization |
| --- | --- | --- |
| `accessibility-auditor` | Investigator | Accessibility inspection |
| `accessibility-fixer` | Implementer | Accessibility remediation |
| `architect` | Investigator | System architecture |
| `backend-dev` | Implementer | Server behavior |
| `build-verification` | Verifier | Build and release verification |
| `code-reviewer` | Reviewer | Independent review |
| `data-engineer` | Implementer | Data operations |
| `database-architect` | Investigator | Database design |
| `debugger` | Implementer | Reproduction and repair |
| `devops-engineer` | Implementer | Operations and deployment |
| `explorer` | Investigator | Source and evidence discovery |
| `frontend-dev` | Implementer | Browser application behavior |
| `fullstack-dev` | Implementer | End-to-end application behavior |
| `general` | Implementer | Cross-domain work |
| `mobile-dev` | Implementer | Mobile application behavior |
| `performance-engineer` | Investigator | Performance analysis |
| `planner` | Investigator | Planning and sequencing |
| `qa-engineer` | Implementer | Tests and regression coverage |
| `refactorer` | Implementer | Behavior-preserving refactoring |
| `security-auditor` | Investigator | Security inspection |
| `senior-consultant` | Consultant | Consultation from retained evidence |
| `technical-writer` | Implementer | Technical documentation |
| `ux-designer` | Investigator | User experience design |

The runtime still authorizes the authenticated role, profile and stage.
Read-only roles retain their current project, tool, and report boundaries.
Writers use their existing authorization, reservations and consent routes.
The Reviewer, Verifier and Consultant remain runtime-reserved. The runtime
runs verification commands and schedules mandatory independent review under its
existing rules. A planned security or accessibility investigation cannot take
the place of independent review.

## Plan, routing and generated delivery

The plan tool advertises the bundled competencies from
[agent-sources/competencies](../../agent-sources/competencies). Each ordinary
node in a fresh plan or replan makes an explicit choice. A nonempty selection
names the relevant modules. An empty selection gives a concrete, concise reason
why the base profile can do the work. Intake checks the shape and canonically
orders the IDs. It retains the exact rationale without scoring its quality.
The worker brief and independent review packet include the accepted choice.
The reviewer assesses fit against the objective and work evidence.
The security and database modules preserve the higher planning risk that the
former audit and database architecture roles carried. Other selections leave
that role-based risk rule alone.
New review fixes use General with competencies drawn from the original work
and the affected files. The subsequent reviewer gets exact repair selections
along with the original choices. A reason inherited for a repair remains
identified as part of the runtime's repair decision. For a retained execution
without an explicit selection, the runtime attaches the legacy role's default
competencies. Steering can keep a historical role on its existing node. An
untouched node preserves its original selection or its historical absence.
An added or edited node makes an explicit choice. Newly selected roles use the
current bases.

For new execution roles, the starting automatic tier follows the complexity of
the work. Low starts at light, normal at standard, and high at
deep. Owner-confirmed selections, pins, hard eligibility, provider availability,
effort policy and review independence still govern the final decision.
Work kind still drives affinity and preferred models. The existing qualified
launch path embeds the selected competency text in the instruction bundle and
preserves the role-specific tool catalogue.

This migration needs no store format change, route file rewrite or owner
settings change. Historical role identities remain readable through the runtime mappings. Add shared behavioral guidance to its base or
protocol, and discipline guidance to a competency. Update the advertised
selector and validation together when adding a competency ID. Render the
existing skill paths before refreshing the portable instruction bundle.

```sh
scripts/cortex-xtask.sh profiles write
scripts/cortex-xtask.sh generate-instructions write
scripts/cortex-xtask.sh check-generated
scripts/cortex-xtask.sh package stamp
scripts/cortex-xtask.sh package check
```

## Skill delivery evidence

The runtime records `NATIVE_SKILL_DELIVERED` after authenticating the exact task,
attempt, session, channel and assignment and preparing a successful inline
tool response. A batch can have successful items alongside item errors. Those
successful items retain evidence only when included in the returned envelope.
A rejected request, authority failure, or envelope size refusal produces no
skill delivery receipt. Repeated inline responses for the same skill and body
within one authenticated channel share one durable receipt.

An oversized batch item receives `NATIVE_SKILL_DEFERRED` with the retained
output identity, digest and byte count. Repeating the identical deferred response
in the same channel reuses that output. The same channel must obtain contiguous
integrity-verified byte pages from offset zero through the final byte for a
`NATIVE_SKILL_DELIVERED` readback event. Duplicate or out-of-sequence pages do
not advance coverage. Another channel cannot read the deferred skill output,
including a replacement attempt after restart.

Each receipt identifies the bundled skill or bundled profile, or uses a
content-independent digest of the canonical project-relative skill path.
The version is `sha256:<body digest>`, a content revision. The body size and
digest cover the UTF-8 skill body actually selected for response. The receipt
keeps no skill body, raw project-skill path or native tool arguments. The
gateway evidence establishes the selected response and completed page coverage.
It cannot establish that the downstream engine displayed, understood or followed
the skill. A permission allowance has its own observation and carries no
skill-delivery evidence.

The existing `NATIVE_ATTEMPT_DELIVERY` event records the assembled competency
module IDs alongside the instruction-bundle digest. Each entry carries the
content revision of the normalized module source and the digest and byte lengths
of both source and rendered section. The runtime computes these values from the
strings it appends while composing instructions. The event records composition
and the intended prompt delivery. Use the session and capsule delivery
observations to establish transport progress.
