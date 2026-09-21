# Advisory lane

The advisory lane is an optional coordinator-owned consultation. It activates
only when the configured advisory provider and exact tool identity are present
and the coordinator has a concrete uncertainty or consequence trigger.

The advisor receives a bounded question, current requirements, selected report
IDs and artifact versions. It can publish its own report but cannot inspect the
project, run commands, spawn workers, change the pipeline or accept work.

Advice is evidence for the coordinator, never an approval gate or a required
stage. Missing, unrelated or unavailable providers are recorded as an absent
optional capability and do not alter ordinary orchestration.
