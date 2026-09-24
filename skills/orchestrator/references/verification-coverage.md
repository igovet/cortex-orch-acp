# Verification coverage

Plan verification for every surface the task changes before submitting the plan,
and make it real, not only unit tests:

- Plugin or UI changes: a worker task that observes the change in the isolated
  dev Paseo GUI as AGENTS.md describes, with the plugin path given explicitly.
- Install or packaging changes: a real install from the documented source into
  a disposable environment.
- Runtime, protocol or engine changes: the documented tests plus, when the user
  authorized it, the documented paid probe or live run.
- Documentation: commands and links checked against the source.

Name each such check in the worker objective so the worker records it.

When a check needs authorization (paid engine calls, pushing a branch, a remote
install), ask the user once, early, with user_question_request while planning,
listing every authorization the plan needs. Do not leave a gap you could have
asked about.

While reconciling, close any gap that the environment and authorizations still
allow with replan_request. List in synthesis only what a real blocker makes
impossible, each item with that blocker; when everything was verified, the final
message has no not-verified list. Never report an unverified item as passed.
