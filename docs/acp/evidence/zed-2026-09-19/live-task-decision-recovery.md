# Direct-project completion required steering — 2026-09-20

Status: **FAIL** for autonomous first-turn completion on source identity
`1.0.0+sha256.e092b028b6260e76`; **PASS** for the explicitly steered
recovery of the same task. The test used the disposable visible Zed 1.20.2
profile and `/tmp/cortex-acp-live-ypkwriti/final/direct-mode-retry-20260919`.
Screenshots stay outside the repository because the UI displays account data.

The baseline unit test failed `7 != 6`. A real Codex Luna high worker ran in
the exact open project, changed `counter.py` to return `sum(values)`, and
published report `r_23f15ac038da`. Its result publication recorded an
observed tree revision before the result event. An independent run of
`PYTHONDONTWRITEBYTECODE=1 python3 -B -m unittest -v test_counter_unittest.py`
passed in that project. The previous overlap guard failure did not recur.

The coordinator recorded two assignment-level accept decisions but omitted
the separate task-level accept decision. Cortex correctly blocked task
`t_c9d1e79ce3b0` with `acceptance_evidence_missing`; the completion gate
reported no missing worker evidence. The task was then continued in the same
Zed thread with a concrete instruction to recheck the result and record an
overall task decision if valid. The coordinator recorded
`decide(assignment_ref="task", decision="accept")`, and the durable task
state became `accepted`. The first autonomous attempt remains a failure;
the later accepted state is evidence for recovery after steering, not for
the coordinator doing the right thing unaided.

The following source revision clarifies the distinct decision scopes in
the coordinator role, the `decide` schema and its assignment-level receipt.
A fresh task without steering must qualify that revision.
