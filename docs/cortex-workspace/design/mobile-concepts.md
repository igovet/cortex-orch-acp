# Mobile concepts

**Status: planned; no implementation or qualification is implied.**

After Gate D, build two clickable mock-only navigation concepts under the proposed `prototypes/cortex-workspace/mobile/` location, using the visual direction selected at Gate D and [shared synthetic fixtures](prototype-fixtures.md). A React Native prototype is provisional: it does not settle the production Android stack, which requires the CW-F02a and CW-F02b device spikes and the CW-F02 go/no-go record. Do not connect to a production backend or use credentials.

- **Now-first:** open on a decision queue and away summary, then task discussion, permission, result/diff and project navigation.
- **Task-first:** open on grouped tasks with an obvious decisions entry, then the same discussion, permission, result/diff and project navigation.

In both concepts include offline and reconnect, stale decision, failed check, file conflict and Git conflict. Distinguish a permission from artifact acceptance; an offline action must not appear committed. Use fixture-only desktop → phone → desktop navigation. Exercise 360×800 and 412×915 logical viewports, large text, touch targets and keyboard navigation; record screenshots and actual results, including failures and unrun device checks. Record selection or requested revisions at [Gate M](mobile-approval.md); absent or conditional approval does not pass it.
