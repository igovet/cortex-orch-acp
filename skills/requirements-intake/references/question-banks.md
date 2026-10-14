# Situational question banks

Use only unanswered material questions. These are prompts, not an interview to
run in full. Ground selectable answers in the project and retain free text.
For each kind cover the seven dimensions below only where they are unresolved.

## Feature / product
- Goal: Which user outcome should this change enable, and why now?
- Users: Who uses it and what roles or permissions differ?
- Scope/non-goals: Which flows are included; what must remain unchanged?
- Acceptance: What concrete example and failure behavior demonstrate success?
- Constraints: Which APIs, compatibility, data, deadlines or dependencies are fixed?
- Risks: What privacy, security, migration or concurrency consequences matter?
- Verification: Which tests, environments and user-visible evidence are required?

## Bug / incident
- Goal: What is expected versus observed, and what impact needs stopping?
- Users: Who is affected and in which environment/version?
- Scope/non-goals: Fix the root cause, mitigate, or diagnose only; what is excluded?
- Acceptance: What minimal reproduction should change and what must still work?
- Constraints: What reproduction data can be shared safely; what access is authorized?
- Risks: Is there data loss, exposure or production impact; which actions are prohibited?
- Verification: Which reproduction, regression and recovery checks prove the fix?

## Refactor
- Goal: Which maintainability or measurable technical outcome is needed?
- Users: Which consumers and maintainers depend on this boundary?
- Scope/non-goals: Which modules may change; are public behavior/schema changes excluded?
- Acceptance: Which invariants and observable interfaces must be preserved?
- Constraints: Which compatibility, dependency and performance limits apply?
- Risks: What migration, concurrency or rollback risks need protection?
- Verification: Which equivalence, contract and performance checks are expected?

## UI / UX design
- Goal: What product outcome and primary user job should the design support?
- Users: Who is the audience, including accessibility and localization needs?
- Scope/non-goals: Which screens, content, states and journeys; what is not being built?
- Acceptance: What observable usability and visual criteria define success?
- Constraints: Which platforms, brand/tokens, references and avoid list are fixed?
- Risks: What missing content, asset rights, interaction or accessibility risks matter?
- Verification: Which mobile/tablet/desktop sizes, states and rendered evidence are needed?
Use the design-workflow bank for detailed visual-direction choices.

## Documentation
- Goal: What should readers understand or accomplish?
- Users: Which audience, prior knowledge, language and reading context?
- Scope/non-goals: Which documents are in scope; what must not be rewritten?
- Acceptance: Which examples or reader journeys must be accurate and complete?
- Constraints: Which source of truth, format, terminology and commands are authoritative?
- Risks: What stale claims, private content or unsafe instructions must be excluded?
- Verification: Which links, commands, examples and source facts must be checked?

## Infra / ops
- Goal: What operational outcome is needed, on which exact target?
- Users: Who depends on it, and who can authorize the action?
- Scope/non-goals: Which isolated resources may change; is production excluded?
- Acceptance: Which health, reliability or recovery observation proves success?
- Constraints: What access, budget, environment, maintenance window and policy apply?
- Risks: What downtime, data loss, credential or rollback risks exist?
- Verification: Which dry run, isolated trial, health check and rollback proof are required?
Never infer authorization from a requested outcome alone.

## Investigation
- Goal: What decision should the investigation enable?
- Users: Who consumes the findings and which users are affected?
- Scope/non-goals: Which hypotheses or systems are included; is remediation excluded?
- Acceptance: What evidence would distinguish explanations or answer the question?
- Constraints: What sources, time, access and confidence limits apply?
- Risks: What sensitive data, disruptive probes or mistaken assumptions must be avoided?
- Verification: Which reproducible observations and disconfirming checks are expected?
