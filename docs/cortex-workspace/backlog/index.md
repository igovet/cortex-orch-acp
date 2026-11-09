# Planned implementation backlog

Gate D passed on 2026-11-05: the owner accepted the clickable browser prototype, whose phases P0 to P6 are built as mock-only, unqualified code. CW-D05 is closed; CW-D08 is not built. Existing first-round mocks are historical. Mobile is deferred by the owner decision of 2026-11-05: CW-M03 (Gate M) gates only the Android and phone-only items and the items that depend on them, and desktop items no longer depend on it. The mock screens for CW-U01 to CW-U15 exist in the prototype to the extent recorded in CW-D02; every production CW-U item remains Planned. The local desktop slice (CW-S01 to CW-S12) is the first implementation; the other milestones follow the [roadmap](../roadmap.md). P05 is post-v1. See [format](format.md) and [roadmap](../roadmap.md).

Each backlog file appears exactly once: 114 active items, 2 archived explorations and 7 retired split parents. The 19 split successors are listed as separate active items (CW-C02a/b, CW-F02a/b, CW-G05a/b, CW-I04a/b/c, CW-Q01a/b, CW-Q02a/b, CW-Q03a/b, CW-Q04a/b, CW-Q06a/b). CW-C02 and CW-F02 remain as S-size roll-up records that depend on their successors. The 12 local slice items CW-S01 to CW-S12 were added on 2026-11-05; the S family is new because the slice crosses the C, F, H, I and U families.

Owner decisions of 2026-11-05 that change backlog items: attachments by drag and drop and paste (CW-R39; CW-U03, CW-U12, CW-U13, CW-U14); automatic sprint moves a checked task to Ready to merge by itself (CW-R20; CW-U05); check recovery and acceptance with an unpassed check (CW-R40; CW-U04, CW-C07); orchestration settings parity and per-tool access on listed MCP tools (CW-R41; CW-U07, CW-C01). Owner decisions of 2026-11-05 also defer mobile (only the Gate M-dependent mobile and Android items wait for it) and make the local desktop slice the first implementation; toolchain prerequisites are in [platforms](../platforms.md). Owner decisions of 2026-11-05 on builds and channels: native builds run in a container and need no host development packages (CW-S09, CW-S11, CW-F01); dev and release channels are isolated, and manual GitHub Actions runs publish production from `main` and dev prereleases from `dev` (owner decision 2026-11-06) (CW-R42; CW-S11, CW-S12, CW-U16). Owner decision of 2026-11-05: desktop release qualification is separate from mobile. The first release is the Ubuntu desktop and Android follows later; the mobile qualification items are CW-Q01b, CW-Q02b, CW-Q03b, CW-Q04b and CW-Q06b, all gated by Gate M. See the [requirements](../requirements.md) owner decisions.

## Active items (114)

| ID | Goal | Phase | Dependencies | Size | Status |
|---|---|---|---|---|---|
| [CW-A01](CW-A01.md) | Android app shell and Now summary in React Native (provisional stack) | 3+ | CW-C04, CW-N07, CW-F02 | L | Planned |
| [CW-A02](CW-A02.md) | Android decision detail, discussion and desktop-synchronized outcomes | 3+ | CW-A01, CW-C03, CW-C08, CW-P02 | L | Planned |
| [CW-A03](CW-A03.md) | Android result screens and file-edit WebView host | 3+ | CW-A02, CW-C07, CW-I05 | L | Planned |
| [CW-A04](CW-A04.md) | Android credential storage and measured background connection | 3+ | CW-A01, CW-N03, CW-N04 | M | Planned |
| [CW-C01](CW-C01.md) | Versioned schemas, capabilities and generated TS/Python contract fixtures | 3+ | CW-F04 | L | Planned |
| [CW-C02](CW-C02.md) | Roll-up: public project outbox and atomic snapshot cursor | 3+ | CW-C02a, CW-C02b | S | Planned |
| [CW-C02a](CW-C02a.md) | Public project outbox appended in the same transaction as runtime state changes | 3+ | CW-C01 | M | Planned |
| [CW-C02b](CW-C02b.md) | Atomic snapshot cursor, bounded replay, gap detection and resync | 3+ | CW-C02a | L | Planned |
| [CW-C03](CW-C03.md) | Decision CAS/idempotency/revocation tests and handler | 3+ | CW-C01, CW-C02 | L | Planned |
| [CW-C04](CW-C04.md) | Typed task/execution/wait/decision read projections with explicit unknown states | 3+ | CW-C01 | L | Planned |
| [CW-C05](CW-C05.md) | Result/evidence completeness and digest-current projection | 3+ | CW-C04 | L | Planned |
| [CW-C06](CW-C06.md) | Stable work-item/execution links and idempotent legacy import | 3+ | CW-C01, CW-F04 | L | Planned |
| [CW-C07](CW-C07.md) | Artifact/version-bound human acceptance separate from runtime completion | 3+ | CW-C05, CW-C06 | L | Planned |
| [CW-C08](CW-C08.md) | Structured steer/stop/recheck; no Paseo-only control dependency | 3+ | CW-C03, CW-C06 | L | Planned |
| [CW-D01](CW-D01.md) | Revised synthetic desktop fixtures | 1 | CW-D06a, CW-D06b, CW-D06c, CW-D07 | S | Mock built (cw-fixtures-v3); unqualified |
| [CW-D02](CW-D02.md) | Revised single desktop direction based on A | 1 | CW-D01, CW-DM02, CW-D07 | L | Mock P0–P6 built; unqualified; Gate D passed 2026-11-05 |
| [CW-D05](CW-D05.md) | Desktop evaluation and owner Gate D record | 1 | CW-D02, CW-D07, CW-D08 | M | Closed; Gate D passed 2026-11-05; P7 checks other than owner acceptance not recorded as run |
| [CW-F01](CW-F01.md) | Pin toolchain versions and stand up the real Ubuntu Tauri 2 + SolidJS + TypeScript shell | 3+ | CW-D05 | M | Planned; a narrow subset is built by CW-S01 and CW-S09 |
| [CW-F02](CW-F02.md) | Roll-up: Android stack go/no-go record from CW-F02a and CW-F02b | 3+ | CW-F02a, CW-F02b | S | Planned |
| [CW-F02a](CW-F02a.md) | Tauri 2 Android minimal spike on the owner's physical device: FCM push, Keystore round-trip and jank capture | 3+ | CW-M03, CW-F01 | L | Planned |
| [CW-F02b](CW-F02b.md) | React Native (New Architecture) minimal spike on the owner's physical device: FCM push, Keystore round-trip and jank capture | 3+ | CW-M03 | L | Planned |
| [CW-F03](CW-F03.md) | CodeMirror 6 (merge and LSP client) and xterm.js (WebGL with canvas fallback) fixture inside the real Tauri webview | 3+ | CW-F01 | L | Planned |
| [CW-F04](CW-F04.md) | Validate the current Cortex runtime's workspace paths and attempt lifecycle against source, as input to the Rust service's file, Git and PTY ownership boundary | 3+ | CW-D05 | L | Planned |
| [CW-F05](CW-F05.md) | WebKitGTK GPU/session startup mitigation layer with owner override and logged diagnostic | 3+ | CW-F01 | M | Planned |
| [CW-G01](CW-G01.md) | Git read projection through git2-rs | 3+ | CW-I01 | M | Planned |
| [CW-G02](CW-G02.md) | Version-checked stage and unstage, with CLI-backed commit | 3+ | CW-G01, CW-I02 | L | Planned |
| [CW-G03](CW-G03.md) | Branch switch, fetch and pull through the Git CLI | 3+ | CW-G02 | L | Planned |
| [CW-G04](CW-G04.md) | Confirmed push and ambiguous-outcome reconciliation | 3+ | CW-G03 | L | Planned |
| [CW-G05a](CW-G05a.md) | Desktop Git panel | 3+ | CW-G04, CW-U02 | L | Planned |
| [CW-G05b](CW-G05b.md) | Android Git screens | 3+ | CW-G04, CW-A02 | M | Planned |
| [CW-H01](CW-H01.md) | Single machine-service instance and ACP child ownership independent of UI | 3+ | CW-F01, CW-F04 | L | Planned |
| [CW-H02](CW-H02.md) | Project registration and authenticated local RPC; explicit Cortex activation | 3+ | CW-H01, CW-C01 | M | Planned |
| [CW-H03](CW-H03.md) | Bounded fan-out, history paging and slow-client resync | 3+ | CW-H02, CW-C02 | L | Planned |
| [CW-H04](CW-H04.md) | Crash, EOF and owned-child reconciliation with no prompt replay | 3+ | CW-H01, CW-C08 | L | Planned |
| [CW-I01](CW-I01.md) | Canonical file tree, read and search in the Rust machine service | 3+ | CW-F04, CW-H02 | L | Planned |
| [CW-I02](CW-I02.md) | Human-mutation lease and writer quiescence | 3+ | CW-C08, CW-F04 | L | Planned |
| [CW-I03](CW-I03.md) | Hash-checked save, create and rename | 3+ | CW-I01, CW-I02 | L | Planned |
| [CW-I04a](CW-I04a.md) | Desktop CodeMirror 6 editor with tabs and find/replace | 3+ | CW-F03, CW-I03 | M | Planned |
| [CW-I04b](CW-I04b.md) | Python language server integration: completion, diagnostics and go-to-definition | 3+ | CW-I04a, CW-F03, CW-H02 | L | Planned |
| [CW-I04c](CW-I04c.md) | TypeScript language server integration | 3+ | CW-I04b | M | Planned |
| [CW-I05](CW-I05.md) | Android small-file edit contract for the CodeMirror 6 WebView bridge | 3+ | CW-A02, CW-I03, CW-F02 | L | Planned |
| [CW-M01](CW-M01.md) | Two mobile navigation concepts using tokens selected at Gate D | 2 | CW-D05 | L | Planned |
| [CW-M02](CW-M02.md) | Mobile decisions/result/reconnect states, keyboard and large text | 2 | CW-M01 | L | Planned |
| [CW-M03](CW-M03.md) | Mobile file/Git conflict flow and desktop return; owner Gate M | 2 | CW-M02 | M | Planned |
| [CW-N01](CW-N01.md) | Embed the iroh Endpoint in the desktop service; persistent NodeId key in the OS keyring; reject debug listeners on 0.0.0.0 | 3+ | CW-F01, CW-H02 | M | Planned |
| [CW-N02](CW-N02.md) | Integrate iroh Android bindings through the selected mobile stack; device key in Android Keystore | 3+ | CW-F02 | L | Planned |
| [CW-N03](CW-N03.md) | QR/link enrollment with one-use secret and local desktop confirmation; replay and expiry rejection tests | 3+ | CW-N01, CW-N02 | L | Planned |
| [CW-N04](CW-N04.md) | Device allow-list and immediate revocation at QUIC/ALPN accept; force-close on revoke | 3+ | CW-C02, CW-N03 | M | Planned |
| [CW-N05](CW-N05.md) | Self-hosted iroh relay with RelayMap configuration and owner-selected access policy; n0 public relay disabled by default as an owner-gated fallback; forced-relay ciphertext-only test | 3+ | CW-N01 | M | Planned |
| [CW-N06](CW-N06.md) | LAN fast path: mDNS advertise and NsdManager discovery feeding a direct dial that bypasses the relay | 3+ | CW-N01, CW-N02, CW-N04 | M | Planned |
| [CW-N07](CW-N07.md) | Network-change resilience: forced clean reconnect and cursor resume on Wi-Fi/cellular handover | 3+ | CW-H03, CW-N03 | L | Planned |
| [CW-P01](CW-P01.md) | Push gateway as a self-hosted container (FCM credential as a Docker secret, caller authentication, metadata-only schema enforced) | 3+ | CW-C02 | M | Planned |
| [CW-P02](CW-P02.md) | Android FirebaseMessagingService integration; on-open encrypted content fetch; token registration and rotation | 3+ | CW-N02, CW-N03, CW-N04, CW-P01 | L | Planned |
| [CW-P03](CW-P03.md) | Desktop event-outbox writes wired to push dispatch with the recipient's stored FCM token | 3+ | CW-C02, CW-N04, CW-P01, CW-P02 | M | Planned |
| [CW-P04](CW-P04.md) | No-GMS device detection and explicit "push unavailable" state; documented limitation | 3+ | CW-P02 | S | Planned |
| [CW-P05](CW-P05.md) | Optional UnifiedPush receiver support for owner-run distributors; additive and opt-in | Post-v1 | CW-P02 | M | Planned |
| [CW-P06](CW-P06.md) | Self-hosted Docker Compose stack for iroh-relay and push-gateway: compose file, env template without secrets, TLS, health checks, deploy, upgrade and rollback runbook | 3+ | CW-N05, CW-P01 | M | Planned |
| [CW-Q01a](CW-Q01a.md) | Deterministic fake-ACP desktop scenario | 3+ | CW-U01, CW-U03, CW-U04, CW-H04 | M | Planned |
| [CW-Q01b](CW-Q01b.md) | Fake-ACP phone leg of the scenario; mobile, gated by Gate M | 3+ | CW-Q01a, CW-A03, CW-H04, CW-U04 | M | Planned; mobile |
| [CW-Q02a](CW-Q02a.md) | Desktop fault and race matrix: network drop, service kill, conflicting decisions, drift and ambiguous push | 3+ | CW-Q01a, CW-I03, CW-G04, CW-G05a, CW-H04 | L | Planned |
| [CW-Q02b](CW-Q02b.md) | Phone and device fault rows: revoked device, drift, conflicting decisions and handover; mobile, gated by Gate M | 3+ | CW-Q01b, CW-Q02a, CW-I05, CW-G05b, CW-N04, CW-N07 | M | Planned; mobile |
| [CW-Q03a](CW-Q03a.md) | Rendered Ubuntu accessibility and long-history checks | 3+ | CW-I04a, CW-I04b, CW-I04c, CW-Q02a | L | Planned |
| [CW-Q03b](CW-Q03b.md) | Physical-Android accessibility and long-history checks | 3+ | CW-Q02b, CW-T02 | L | Planned; mobile |
| [CW-Q04a](CW-Q04a.md) | Ubuntu installer, service and file/RPC authorization checks | 3+ | CW-Q03a | L | Planned |
| [CW-Q04b](CW-Q04b.md) | Android APK upgrade checks | 3+ | CW-Q03b | M | Planned; mobile |
| [CW-Q05](CW-Q05.md) | Desktop migration and rollback rehearsal; no production Paseo mutation | 3+ | CW-C06, CW-Q04a | M | Planned |
| [CW-Q06a](CW-Q06a.md) | Ubuntu pilot checklist and desktop qualified-support statement | 3+ | CW-Q03a, CW-Q04a, CW-Q05 | M | Planned |
| [CW-Q06b](CW-Q06b.md) | Android pilot checklist and Android qualified-support statement; mobile, gated by Gate M | 3+ | CW-Q06a, CW-Q01b, CW-Q02b, CW-Q03b, CW-Q04b | M | Planned; mobile |
| [CW-T01](CW-T01.md) | Service-owned PTY sessions | 3+ | CW-F03, CW-I02 | L | Planned |
| [CW-T02](CW-T02.md) | Android advanced terminal attachment | 3+ | CW-A02, CW-T01 | M | Planned |
| [CW-U01](CW-U01.md) | Desktop board, list and task creation | 3+ | CW-C04, CW-H02 | L | Planned |
| [CW-U02](CW-U02.md) | Desktop task detail: discussion, plan, workers and bounded history | 3+ | CW-C06, CW-H03, CW-U01 | L | Planned |
| [CW-U03](CW-U03.md) | Desktop decision queue with explicit typed conflicts | 3+ | CW-C03, CW-U01 | M | Planned |
| [CW-U04](CW-U04.md) | Versioned result screen on the CodeMirror merge view | 3+ | CW-C07, CW-F03, CW-U02 | M | Planned |
| [CW-DM01](CW-DM01.md) | Workspace ubiquitous language and domain invariants | 1 | None | M | Partly built in mock domain; docs not complete |
| [CW-DM02](CW-DM02.md) | Bounded contexts and runtime translation | 1 | CW-DM01 | M | Partly built; bounded-context document and ADR 0012–0013 absent |
| [CW-D06a](CW-D06a.md) | Prototype task domain and lifecycle tests | 1 | CW-DM01 | L | Mock built; unqualified |
| [CW-D06b](CW-D06b.md) | Prototype sprint domain and queue tests | 1 | CW-D06a | L | Mock built; unqualified |
| [CW-D06c](CW-D06c.md) | Prototype archive domain tests | 1 | CW-D06a | M | Mock built; unqualified |
| [CW-D07](CW-D07.md) | Prototype port and contract test harness | 1 | CW-D06a, CW-D06b, CW-D06c | M | Partly built: port and mock adapter; golden mappings not built |
| [CW-D08](CW-D08.md) | Rendered prototype review harness | 1 | CW-D02 | M | Planned; not built |
| [CW-S01](CW-S01.md) | Local slice prerequisites: Rust toolchain pin, Cargo skeleton and document reconciliation | 3+ | CW-D05 | S | Built; no-paid checks pass |
| [CW-S02](CW-S02.md) | Shared Workspace wire subset, generated contract and scripted protocol peer | 3+ | CW-S01 | L | Built; no-paid checks pass |
| [CW-S03](CW-S03.md) | Promote the accepted presentation behind an asynchronous production seam | 3+ | CW-S02 | L | Built; no-paid checks pass |
| [CW-S04](CW-S04.md) | Python draft and execution identity with authoritative read projections | 3+ | CW-S02 | L | Built; no-paid checks pass |
| [CW-S05](CW-S05.md) | Transactional invalidation journal and bounded snapshot reads | 3+ | CW-S04 | L | Built; no-paid checks pass |
| [CW-S06](CW-S06.md) | Idempotent explicit start and read-only result projection | 3+ | CW-S04, CW-S05 | L | Built; no-paid checks pass |
| [CW-S07](CW-S07.md) | Identified single and batch question answers | 3+ | CW-S05, CW-S06 | L | Built; no-paid checks pass |
| [CW-S08](CW-S08.md) | Rust service ownership, authenticated local RPC and child supervision | 3+ | CW-S02, CW-S04, CW-S05, CW-S06, CW-S07 | L | Built; no-paid checks pass; different-OS-user check unverified |
| [CW-S09](CW-S09.md) | Real Tauri shell with a closed native bridge | 3+ | CW-S03, CW-S08 | L | Built; native build and journey pass in the container build |
| [CW-S10](CW-S10.md) | Connect the first owner journey and hide every unsupported control | 3+ | CW-S06, CW-S07, CW-S08, CW-S09 | L | Built; native journey passes in the container build with fake engines |
| [CW-S11](CW-S11.md) | Local packaging, user service unit and release handoff | 3+ | CW-S10 | M | Built; .deb content checks pass; user-unit start and stop unverified; GitHub run not performed |
| [CW-S12](CW-S12.md) | Isolated dev and release channels, dev settings clone and project refusal rule | 3+ | CW-S11 | M | Built; dev package and fake-engine journey pass in the container build; release channel not published |
| [CW-C09](CW-C09.md) | Sprint planning and scheduler contract | 3+ | CW-C01, CW-C06, CW-D06b | L | Planned |
| [CW-C10](CW-C10.md) | Archive retention and guarded purge contract | 3+ | CW-C06, CW-D06c | L | Planned |
| [CW-C11](CW-C11.md) | Description-first generated title contract | 3+ | CW-C01 | M | Planned |
| [CW-C12](CW-C12.md) | Durable worker message capture and delivery | 3+ | CW-C03, CW-C06, CW-C08 | L | Planned |
| [CW-C13](CW-C13.md) | Interrupt-now worker message per engine; enabled only for qualified engines | 3+ | CW-C08, CW-C12 | M | Planned |
| [CW-G06](CW-G06.md) | Optional worktree lifecycle | 3+ | CW-F04, CW-I02, CW-D06a | L | Planned |
| [CW-G07](CW-G07.md) | Owner-confirmed merge and PR | 3+ | CW-G06, CW-C07, CW-G03 | L | Planned |
| [CW-G08](CW-G08.md) | Per-attempt change attribution | 3+ | CW-G01, CW-G06, CW-I02 | M | Planned |
| [CW-H05](CW-H05.md) | Machine registry and connector | 3+ | CW-H02 | L | Planned; after local slice |
| [CW-U05](CW-U05.md) | Sprint planning and execution UI | 3+ | CW-C09, CW-U01 | L | Planned |
| [CW-U06](CW-U06.md) | Archive and purge UI | 3+ | CW-C10, CW-U01 | M | Planned |
| [CW-U07](CW-U07.md) | Per-machine orchestration settings | 3+ | CW-H05, CW-U01, CW-U10 | L | Planned; after local slice |
| [CW-U08](CW-U08.md) | Command and lifecycle controls | 3+ | CW-C08, CW-U02, CW-U13 | L | Planned |
| [CW-U09](CW-U09.md) | Machine switcher and aggregate views | 3+ | CW-H05, CW-U01 | M | Planned; after local slice |
| [CW-U10](CW-U10.md) | Task composer and session controls: model, effort, access mode, language, context and usage | 3+ | CW-U02, CW-C01, CW-C04 | L | Planned |
| [CW-U11](CW-U11.md) | Voice input for the task composer; speech provider is an open question | 3+ | CW-U10, CW-U07, CW-U15 | M | Planned; after local slice |
| [CW-U12](CW-U12.md) | Task composer Steer and Queue send modes | 3+ | CW-U02, CW-U07, CW-C08 | M | Planned; after local slice |
| [CW-U13](CW-U13.md) | Task Workers tab: worker list, live and finished worker views | 3+ | CW-U02, CW-C04, CW-C05, CW-H03 | L | Planned |
| [CW-U14](CW-U14.md) | Worker message composer and delivery states | 3+ | CW-C12, CW-U13 | M | Planned |
| [CW-U15](CW-U15.md) | Machine dictation service: streaming protocol, local models and OpenAI-compatible provider | 3+ | CW-U07, CW-H05, CW-C01 | L | Planned; after local slice |
| [CW-U16](CW-U16.md) | Desktop application self-update from GitHub releases (.deb): detection, verified install, failure and rollback states | 3+ | CW-F01, CW-H01, CW-D05 | L | Partial source; privileged and signed-update qualification open |

## Archived first-round explorations (2)

| ID | Goal | Phase | Dependencies | Size | Status |
|---|---|---|---|---|---|
| [CW-D03](CW-D03.md) | Historical desktop B concept | 1 | None | L | Archived; unqualified |
| [CW-D04](CW-D04.md) | Historical desktop C concept | 1 | None | L | Archived; unqualified |

## Retired split parents (7)

These files are redirect notes so existing links resolve. They are not implementable, must not be used as dependencies, and are not Planned. Reference the successors listed in each file.

| ID | Goal | Phase | Dependencies | Size | Status |
|---|---|---|---|---|---|
| [CW-G05](CW-G05.md) | Retired; split into CW-G05a and CW-G05b | 3+ | None (see successors) | none | Retired |
| [CW-I04](CW-I04.md) | Retired; split into CW-I04a, CW-I04b and CW-I04c | 3+ | None (see successors) | none | Retired |
| [CW-Q01](CW-Q01.md) | Retired; split into CW-Q01a and CW-Q01b | 3+ | None (see successors) | none | Retired |
| [CW-Q02](CW-Q02.md) | Retired; split into CW-Q02a and CW-Q02b | 3+ | None (see successors) | none | Retired |
| [CW-Q03](CW-Q03.md) | Retired; split into CW-Q03a and CW-Q03b | 3+ | None (see successors) | none | Retired |
| [CW-Q04](CW-Q04.md) | Retired; split into CW-Q04a and CW-Q04b | 3+ | None (see successors) | none | Retired |
| [CW-Q06](CW-Q06.md) | Retired; split into CW-Q06a and CW-Q06b | 3+ | None (see successors) | none | Retired |

## Dependency graph checks

- Every dependency named in the table is an active item. None points at a retired parent.
- The intended graph is acyclic: domain and prototype prerequisites lead to CW-D05. The local slice chain is CW-S01 → CW-S02 → (CW-S03, CW-S04) → CW-S05 → CW-S06 → CW-S07 → CW-S08 → CW-S09 → CW-S10 → CW-S11 → CW-S12; CW-S06 and CW-S07 also feed CW-S08.
- CW-M03 is a direct dependency only of CW-F02a and CW-F02b. It gates the Android and phone-facing items that depend on them, and no desktop item depends on it directly or transitively. The desktop release items CW-Q01a, CW-Q02a, CW-Q03a, CW-Q04a, CW-Q05 and CW-Q06a do not reach CW-M03; the mobile qualification items CW-Q01b, CW-Q02b, CW-Q03b, CW-Q04b and CW-Q06b do.
- Resolved owner decision (2026-11-05): desktop release qualification is separate from mobile. The mixed items CW-Q01, CW-Q02 and CW-Q06 are split so that each desktop part has no mobile edge; the mobile parts are the Gate M-gated successors named above. The split edges keep the graph acyclic: no successor depends on a retired parent, and no active item depends on a retired parent.
- Requirement references in every item exist in [requirements](../requirements.md).
