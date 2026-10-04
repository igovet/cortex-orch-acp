# Live verification

Follow the TARGET project's documented live-check policy, supported commands,
isolation procedure and evidence requirements. Verify capabilities in the current
installed host before relying on them; do not assume a future API or test facility.

Never launch a paid call without an explicit owner request for the current work.
Respect the exact requested mode, route and scope and any stricter project policy.
Do not substitute another route, create accounts, copy credentials or mutate
production without explicit authorization. Use the documented disposable test
environment and keep credentials and raw private logs out of retained evidence.
If authorization is absent, report the concrete policy and missing request,
for example "paid live check not requested by the owner".

Bind every observation to its exact task, session and artifact. Missing,
ambiguous, pending, truncated or replayed evidence cannot establish a pass.
Unit tests, fake sessions and logs support only the behavior they exercise.
Rendered UI claims require inspection of the actual rendered behavior; a
headless check establishes only the runtime or protocol surfaces it exercises.
Report unsupported or unobservable signals and concrete blockers truthfully.
