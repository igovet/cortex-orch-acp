# Pipeline publication and recovery

Read this reference only when creating, editing, publishing or recovering the
pipeline draft.

Use the live Cortex descriptions and schemas. Create one pipeline draft for the
current edition and treat the returned Markdown, draft identifier, path, required
first line and ordered `replaceable_markers` list as authoritative. Preserve the
first line and following blank line. Before `write_report`, replace every exact
listed marker in place and verify that no listed marker (or other exact template
placeholder) remains; do not rely on a remembered heading list or a generic
"completed" substitution. Do not rewrite, rename, move or delete the draft.

Keep document kind aligned with ownership: a coordinator publishes a current
`pipeline` edition, while a worker publishes its own non-pipeline report. Do not
turn a worker report into a pipeline edition or publish a pipeline draft as a
worker report. Publication metadata must describe that same kind and the current
edition/result, not a second document.

Do not assume JavaScript globals inside a `functions.exec` wrapper; the wrapper
may fail before Cortex receives the call. Follow each operation's advertised
retry contract and retain acknowledged receipts.

Edit only the exact pipeline draft path returned to this coordinator. Pass the exact
patch intact to the native patch tool, directly or through a safe host wrapper, and
inspect its complete receipt. An inert JavaScript string form is valid; executable
interpolation or substitution of draft content is not. Never pass the Markdown body
through a Cortex writer or shell command. Publish the completed draft once with
short metadata through the live writer and retain its receipt. An acknowledged
publication is immutable and must not be replayed.

Use the draft reader only after compaction, restart, interrupted editing or a
conflict that makes the retained Markdown uncertain. Follow its bounded cursor until
the exact affected fragment is recovered. If `write_report` returns the deterministic
`draft_guidance_remaining` error, keep this same draft, correct every exact remaining
marker in place, inspect the patch receipt, and retry the same logical publication
according to the live contract. Do not create a replacement
draft or replay an acknowledged publication. For any other uncertain publication,
follow the writer's advertised retry guidance and preserve the existing draft.

The current edition must preserve active and cancelled requirements, decisions,
assignments, dependencies, resource owners, open actions and provenance pointers.
Never remove unresolved work merely to make the pipeline appear complete.
Copy assignment, report, artifact and check identifiers exactly from current
durable receipts. Copy an executed worker engine/model/effort only from that
assignment's current-capsule `observed_worker_route`; a brief or planned `delegate`
route is not execution evidence. Omit or mark unavailable any fact the durable
evidence does not supply instead of reconstructing it from model prose.
For task-level acceptance, publish the edition before `decide(accept)`. Record the
decision rationale, assignment and report references, artifact and check evidence,
remaining work, and the factual basis for the coordinator's intended task-level
decision. Replace the template's objective/decision-basis marker with the objective and
decision basis, without a momentary status. Remaining work means unresolved
deliverables or verification, not the `decide` call itself. Do not say acceptance
was already recorded before the `decide` receipt, or that acceptance is pending,
not yet recorded or the next step. The edition must remain true after `decide`;
readers should consult the durable task decision and state for the recorded outcome.
After `decide`, publish a correcting edition only if the actual decision changed.
