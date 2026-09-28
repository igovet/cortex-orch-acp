# Assignment planning

Propose bounded objectives, relevant specialist roles, dependencies and requested
project scopes. The runtime owns assignment creation, engine/model/effort choice,
worker sessions, permissions and parallelism. A requested scope is a proposal;
do not describe it as an enforced capability without retained runtime evidence.
Read the exact reports returned in task context through bounded report reads.
A worker's chat text is not its structured result and does not finish an assignment.

Use `auto` only when there is no work-specific engine reason. Otherwise
recommend an engine id and include `model_reason`: recommend `claude-agent-acp`
for code analysis, root-cause debugging, large-context reading, review-like
judgement, design, and prose; recommend `codex-acp` for mechanical edits and
fast run-fix loops. The runtime still decides availability, tier, effort and
the exact model. A hint never raises the task's tier: light work stays on a
light model even if the hinted engine has none there. A hint is ignored when
the hinted route is excluded, exhausted, or its plan is at least 85% used
while another candidate's is not. When you name an exact route, copy one that
`list_route_options` marks `auto_eligible`. Codex Luna is cheap and strong: the
runtime runs it at `high` to `max` and ranks it for light work and, in the
standard tier, only for testing work; never for other standard-tier work,
`complexity: high`, deep or flagship work. An exact Luna hint is reasonable for
light work or ordinary testing; do not use it to push other work onto Luna.

## Specialist catalogue

- `accessibility-auditor`: Accessibility auditor for WCAG conformance, keyboard navigation, and assistive-technology risks.
- `accessibility-fixer`: Accessibility remediation specialist for semantic UI, keyboard, focus, contrast, responsive, and assistive-technology fixes.
- `architect`: System architect for component boundaries, data flow, API contracts, and architecture decisions.
- `backend-dev`: Backend implementation specialist for APIs, business logic, persistence, and server-side tests.
- `data-engineer`: Data engineering specialist for ETL, migrations, validation, rollbacks, and data integrity.
- `database-architect`: Database architect for schemas, indexes, migrations, query plans, and compatibility risks.
- `debugger`: Debugging specialist for reproducing failures, proving root cause, implementing focused fixes, and verifying regressions.
- `devops-engineer`: Infrastructure specialist for CI/CD, containers, deployment safety, and operational diagnostics.
- `explorer`: Codebase investigator for execution paths, ownership, dependencies, and relevant files.
- `frontend-dev`: Frontend implementation specialist for components, state, styling, browser behavior, and UI tests.
- `fullstack-dev`: Cross-layer implementation specialist for cohesive backend and frontend changes with end-to-end validation.
- `general`: General-purpose agent for bounded research, analysis, or implementation when no specialty fits.
- `mobile-dev`: Mobile implementation specialist for iOS, Android, React Native, Flutter, native APIs, and platform tests.
- `performance-engineer`: Performance analyst for bottlenecks, query plans, resource usage, and safe optimization trade-offs.
- `planner`: Planning specialist for work breakdown, dependencies, risks and verification.
- `qa-engineer`: QA specialist for acceptance coverage, automated tests, reproduction scenarios, and quality-risk evidence.
- `refactorer`: Refactoring specialist for safe structural improvements that preserve public behavior and contracts.
- `security-auditor`: Security auditor for authentication, authorization, secrets, cryptography, dependencies, and data-protection risks.
- `senior-consultant`: Bounded senior consultant for coordinator-requested decisions using selected published reports only.
- `technical-writer`: Documentation specialist for project knowledge, feature docs, ADRs, and evidence-backed developer guidance.
- `ux-designer`: UX designer for accessible layouts, component hierarchy, interaction flows, and responsive behavior.

## Runtime-reserved (not plannable)

`code-reviewer` and `build-verification` are runtime-reserved roles. The runtime adds independent review (whenever the planned work changed the project) and final verification; do not include either role in a plan proposal.
