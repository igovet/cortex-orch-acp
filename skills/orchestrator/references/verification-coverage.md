# Verification coverage

Plan real verification for every changed surface using the TARGET project's
current documented commands, safety rules and available environment:

- UI or plugin changes: focused type, component and integration checks; observe
  rendered behavior in the project's documented isolated environment when needed
  and authorized. Logs or simulated rendering do not establish a GUI result.
- Installation or packaging changes: the documented install and package checks
  in a disposable environment, including a real install when the contract needs it.
- Runtime, protocol or integration changes: focused regression tests and the
  project's documented smoke or end-to-end checks for the changed boundaries.
- Documentation: check affected commands, links and claims against current source.

Name each check in the worker objective. Workers run focused checks for their
changed surfaces, not the whole suite. Put the documented final suite in the
plan's verification section; the runtime runs final verification after review.
Follow the live proposal schema for command encoding and evidence requirements.

Apply the target project's live-check policy and authorization boundaries.
Never launch a paid call without an explicit owner request for the current work.
A request authorizes only its specified mode, route and scope; do not substitute
another route or touch production unless separately authorized. When a required
request is absent, record the concrete policy and missing authorization, such as
"paid live check not requested by the owner". Do not replan around a prohibition.

While reconciling, request a replan for verification gaps that can be closed in
an authorized environment. Otherwise name the concrete blocker in the structured
result and synthesis. Bind observations to exact, complete, non-replayed evidence;
never infer a pass from a missing or unsupported signal. When every applicable
check is verified, omit a not-verified list.
