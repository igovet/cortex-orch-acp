# Haiku 5.5 live-route recheck — 2026-10-08

## Result

The owner-authorized isolated Paseo desktop recheck did **not** show Haiku 5.5 after a visible Refresh. The rendered Cortex model list remained at six entries (Opus 5.5, Fable 5.1, Sonnet 5, 6.1 Sol, 6 Astra, 6 Luna). The exact assertion for Haiku/alias presence failed (outside-command receipt `gateway:23ad20776770406584537ce8ae090b72`, exit 1); the physical Refresh interaction completed (receipt `gateway:eb1833f9ac01488882e2744689c04aa3`, exit 0).

## Root-cause follow-up

Predecessor report `r_9c324f0b2833` identifies a pre-existing, stale isolated-dev routing file as the cause of the earlier missing route. To test whether that alone explained the result, this recheck selected the disposable test project (visible project selector verified; receipt `gateway:de99b6bc861047248c61deba264c7c51`) and gave it an explicit project routing override enabling exact model ID `haiku` for `claude-agent-acp`, plus `auto_allow` for `claude-agent-acp/haiku`. After a rendered Refresh, Haiku was still absent.

This weakens the claim that stale user-level routing alone explains the UI result. It does **not** establish that the running Claude SDK omitted Haiku: the isolated dev Cortex state contained zero genuine `catalog/cs_*.json` connection snapshots after refresh (receipt `gateway:116454016aca41d3b9a9f7859b09d01a`). The actual engine-returned catalog therefore remains unverified. The visible picker’s Refresh was not evidence of a fresh Claude-engine `session/new` catalog observation.

## Boundaries and artifacts

- Paseo 0.11.1, isolated `~/.paseo-dev` daemon and `~/.config/Paseo-dev` desktop profile; the guarded daemon stop completed without force (receipt `gateway:8e23a2e954c64c91aa3c721ae59ed8f4`) and final status reported stopped (receipt `gateway:c40abcd505ad4bcfa1d99048f5aecfa1`).
- No model prompt/task was sent in the test project. No production Paseo, production plugin, or production routing was changed.
- The temporary screenshot for this recheck was overwritten with a safe navigation-only crop after a relaunched desktop restored the wrong workspace. It is not catalog evidence; rely on the retained rendered-action/assertion receipts above, not `livedesktop-haiku-route-after-override-2026-10-08.png`.
- No project source or production configuration was changed. The only recheck routing override was in the isolated disposable project.

## Next action

The runtime/engine owner must make a genuine prompt-free Claude route-catalog refresh through a supported Cortex engine-refresh surface and retain its real catalog identity, then repeat the desktop check and confirm that `haiku` resolves to Haiku 5.5. Until then the original live-visibility requirement is unmet; do not treat SDK/package metadata or the six-item picker as proof of what a fresh engine response contains.
