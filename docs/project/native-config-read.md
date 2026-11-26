# Native configuration observations and read effects

## Current boundary

`workspace_service::config_read_model` supplies the native passive machine
`config.read` display through `machine::config_read` and `ConfigReadHost`.
It is not a route authorizer. The socket uses the bounded native inspection
lane and has no Python fallback. It does not start a scheduler, acquire a project
owner, or open a project store. Machine `config.save` now uses the separate
[native writer](../configuration.md#native-machine-saves) for routing, defaults
and MCP, without Python fallback.
`config.action` uses [native connection metadata edits](../configuration.md#native-machine-actions)
for rename/session limits, the separate native confirmed default-preset reset,
and profile read/create/save/update/delete/deactivate. Qualified format-1/2 four-section
activation, wider ignored profile-document metadata and absent-only preset application
also run natively. Unqualified catalog/setup, wider modeled fields/backing files, unqualified
activation documents and custom
preset documents still select Python before admission. The separate action adapter
implements filesystem-reference validate and canonical secret-file removal without
reading secret contents; valid keyring validation and removal with unsupported defaults
retain Python; stale-revision keyring refusals are native. This does not widen the passive reader or
change provider admission; see the per-action table linked above. Native lazy Claude quota refresh is not
implemented. Display never runs those producers.

The separate machine `engine.connection` `login_command` action now runs in Rust
on the bounded inspection lane. It selects a qualified channel-local bundle or
owned npm adapter, rechecks install paths/content and returns the native executable,
login argv and adapter argv. It never launches Node, a login/status command or a
model, reads credentials, writes the registry or falls back after a native error.
`recheck` is a separate native action on the action lane: verified host-selected
native status argv, original HOME, filtered child environment, discarded output and
an eight-second timeout, followed by revision-locked native-account create/reuse.
It never reads marker contents or writes native account directories. Existing
references retain their state/timestamps on reuse, as in Python. Native failures
and uncertain registry writes never replay. `add_key` also uses Rust, with separate
create/write/validate registry transactions, canonical private secret-file writes
and known-failure removal/opt-out. Failed cleanup or uncertain replacement remains
unknown, never replayed. Valid keyring validation, unqualified catalog producers and
OpenCode onboarding with a binary candidate remain Python. Pinned Codex/Claude
installation and no-candidate OpenCode onboarding are native
([pinned install](../configuration.md#native-pinned-engine-install)), and every
other `engine.connection` action is a native `unsupported_action` refusal, so that
operation never starts the helper.
No OS-keyring reference is converted into a file or native backend.
Default native
service startup no longer requires Python. Setup bundle lookup uses the trusted
runtime asset root; see
[startup](../cortex-workspace/local-task-execution.md#python-independent-native-service-startup).
Explicit catalog refresh now has a separate
[read-only native prelaunch subset](../configuration.md#native-prelaunch-catalog-refresh):
empty registries, entirely null/empty/relative launch commands, or qualified
[OpenCode pre-probe rejections](../configuration.md#opencode-pre-probe-rejection), an existing
private owner lock and a qualified cached projection. It retains exact failure
outcomes, full cached replies and unchanged configuration/home/catalog trees,
without launching an engine or publishing a snapshot. A separate
[launch-capable producer](../configuration.md#native-launch-capable-catalog-producer)
now qualifies Codex/Claude direct commands, qualified two-argument Codex/Claude
installs and native-account or API-key/secret-file launches. Unicode names/models
and scrubbed noisy logs have Python-oracle coverage. Canonical secret files are
read only for explicit API-key launches; native marker contents stay unread.
Noncanonical references fail without target reads or peer launch. Immutable
snapshots and exact-byte fences retain the shared owner lock and never replay
unknown effects. Sixty-two Python captures (54 prior cases and eight metadata-only
OpenCode failures) compare complete
replies/tree/modes and exact transcripts with zero helper calls.

The exact-hash Codex 2.3.0 bundle is now patched and observed natively; its
adapter is byte-identical to the recorded former patch, and the digest and every patched call site
are still checked before launch. Potentially usable OpenCode installs,
OS-keyrings, unqualified
Codex/Claude candidates (including unsafe artifact state or non-ASCII/control-bearing
bundled-binary paths), unsafe names/roots, missing locks and unqualified projections
retain the whole Python request before effects. Unqualified Claude same-name
model effort fallback and over-bound diagnostics refuse after observation without
snapshot/replay. Redacted lines retain a 400-character prefix plus truncation
metadata, with a 256-KiB file cap; known launch secret echoes are refused. No native keyring backend,
installer or onboarding producer is claimed. The lazy Python Settings provider
is still required. Qualified Codex installs now resolve nested/hoisted npm
dependencies, generate source-guarded private adapter/launcher artifacts and
merge offline debug-model efforts. Their catalog launcher now uses the compiled
Rust service as a native Linux shebang interpreter, not Python or a shell.
Safe legacy Python artifacts migrate atomically on qualified refresh without
replay; existing open files survive replacement. 39 offline execution comparisons
cover argv/environment/cwd/stdio, checks, timeouts/reaping and final PID/status/signals.
The 62 catalog comparisons explicitly validate the changed launcher content;
all other reply/tree/mode/transcript comparisons are unchanged. Unsupported
interpreter paths still retain Python before effects. Separate engine/session
launching is not claimed Python-free. Debug stdout/stderr are never
retained and each is bounded to 4 MiB; overflow/I/O uncertainty refuses replay.
Ordinary optional debug failures retain the ACP effort set. See the linked
producer boundary for captured versions, modes and reproducibility commands.

Manifest reads are bounded to 4 MiB, discovery to 512 directory entries and
content hashing to 512 MiB per regular file; oversized/unstable files refuse
rather than being treated as verified installs. These are native safety limits,
not claims of byte parity for unbounded or unsafe Python inputs.

The library accepts explicit trusted host roots through `RouteEnvironment`,
never roots taken from model/socket JSON. It reads private, bounded JSON through
the existing no-follow native loader, refuses unsafe files and invalid roots,
and does not create missing roots. Missing profiles normalize to an empty
format-2 document; missing connections normalize to an empty schema-1 registry.
Malformed/unsafe files are errors, not successful empty observations.
Profile format-1 conversion is in-memory only. Four profile sections are
normalized in canonical order: routing, user defaults/ceiling, MCP usage policy
(without command/args/env), and connection session limits. Unknown profile
document metadata and unknown routing keys are preserved as in Python.
Library refusal codes are intentionally coarse, not management wire errors.

The registry snapshot retains credential **references**, not credential bytes;
it normalizes legacy omitted concurrency/owned fields but never validates
credentials or writes derived connection state. Parsing a configured record is
not authorization. Route fingerprints hash exactly engine/provider/endpoint/
launch-command, preserving null versus empty argv and Unicode. No fingerprint
is automatically approved.

Low-level adapter observations cover declared unambiguous package bin/version and
current version-policy markers. The setup-composed path below now selects from the
host's channel-local bundle and Cortex-owned npm trees. Neither seam grants launch
trust, instruction-delivery qualification or account validity; neither launches one.
Native-login observations inspect directory/marker metadata only; marker presence
does not prove account validity. Their caller must supply a trusted canonical
native directory: they are not a substitute for canonical-reference, collision
and trust checks at dispatch.

The low-level usage-cache reader consumes the explicit connection cache only,
without CLI refresh; the composed reader below also observes Codex rollouts.
They filter expired/invalid/overage windows and Claude's legacy five-hour window;
unknown use stays unknown, never zero. Exact-model context-window observations
are positive integer cache values only. These advisory observations never make
a route selectable or a task accepted.

## Side-effect contract

The comparison implementation (the former Python `config_read` helper, since removed) called
the management read-all, route/catalog readers, prompt-free connection discovery,
selectable-entry computation and plan usage. Its name is **not**
a guarantee of a pure read. This table distinguishes current effects from the
native-library contract; dropping an incidental effect in the library does
is now an explicit passive machine-read contract, not full side-effect parity.

| Retained operation/effect | Native observation contract and reason |
| --- | --- |
| Registry/config private-root creation, mode repairs and root ignore files through registry reads | Dropped from reads. The native reader never creates or repairs user state; explicit setup/writers must own those effects. |
| Profile format-1 normalization during reading | Ported as a pure in-memory format-2 conversion. No migration backup or profile replacement on read. Explicit machine profile read/create/save/update/delete/deactivate now use the native revision-locked action writer; qualified format-1/2 four-section activation is native, while unqualified documents retain the complete Python journaled workflow. Passive display does not invoke either writer. |
| Connection validation from selector/catalog reads; possible `connections.json` rewrite | Dropped from the observation library. Persisting derived authentication state belongs to an explicit refresh/setup action with registry arbitration, not a display read. Canonical references and duplicate paths still require authoritative dispatch validation. |
| Catalog snapshot/cross-connection fallback, model labels, efforts, route eligibility and source revisions | The catalog-composed native reader now loads genuine snapshots and computes complete cached display catalog/selector observations, labels, efforts and exact-policy diagnostics. It uses stored connection state; incidental validation remains an explicit refresh boundary. Display eligibility never authorizes dispatch. |
| Adapter discovery for Codex/Claude package trees | Native setup display now selects bundle/owned npm candidates and composes passive setup status. Bundle dependency shape/readability/stability checks are display observations, not approved pins or instruction-delivery qualification. No download/install, login or engine model discovery in the library. |
| Optional OpenCode version discovery subprocess | Dropped from reads. No executable, even a version probe, is launched by the library. Explicit contained discovery remains separate. |
| Native login marker existence/type/mode checks | Ported as metadata-only observations; credential contents and keyring are never read. Full canonical reference/collision-derived state remains the writer/dispatch boundary. |
| OS keyring lookup during incidental connection validation | Deliberately unsupported natively. Candidate host display retains stored state with `keyring_not_observed`; an explicit refresh selecting a keyring refuses the entire batch with `keyring_refresh_unsupported`, without any lookup, inferred absence or state rewrite. This explicit unsupported-result decision replaces parity for that effect; machine display uses this passive contract. |
| Claude lazy `claude -p /usage`, quota directory, per-connection lock, probe TTL/negative cache and quota replacement | Deliberately dropped from reads to avoid blocking/process side effects. The library only reads the retained quota cache. A separately qualified producer must refresh it; missing/expired cache is unknown. No native lock/probe/cache file is written. |
| Codex native rollout scan for plan windows | The composed reader now wires the existing bounded read-only rollout observer into settings. It requires the expected private native directory and rejects ambiguous references/symlinked directories; no Codex quota-cache fallback. |
| Context-window cache reads | Ported, exact model keys only. Observation writes belong to engine event handling, not config reads. |
| Workspace defaults, routing precedence, profile modified/interrupted state, revision tokens, MCP view and setup/usage aggregation | Native display projection now maps completed management observations into the settings envelope, including workspace defaults, revisions, profiles, MCP, setup and weekly usage aggregation. The management-composed path below now loads defaults/profile observations and profile status/differences. The Workspace-composed path below now supplies Workspace defaults; setup-composed discovery now supplies passive setup observations. The additional routing-composed path below now loads routing precedence and cached effort observations. |

The native machine dispatcher composes the completed views and delivers advisory
diagnostics. Machine config.action retains Python launch-capable catalog/setup refresh;
native selected-filesystem refresh remains host-only, and native keyring refresh is
unavailable. A read starts the background limits refresh but never waits for it
([plan usage](../features/token-and-plan-usage/index.md#claude-plan-usage-the-local-usage-cli)). No read claims freshness when those
observations are missing. Existing unknown-effect and single-owner fences remain:
a native refusal or partially executed refresh never replays through Python.

Profile action coverage additionally uses 125 real dispatcher captures from
the recorded captures `config-profiles-python.json` and `config-profiles-edges-python.json`
(their Python generators were removed). Complete action replies and all file
names/bytes/modes are compared at the service socket with zero helper calls,
including refusals. Additional real-dispatch captures cover format-1/2 four-section
activation and wider ignored profile-document metadata across all seven verbs in
Rust. Wider modeled/backing-file numbers and unqualified materialization documents
still select Python before admission. The action adapter preserves management diagnostics rather
than substituting the passive reader's deliberately coarse diagnostics. Profile
format-1 writes back up/verify/restore; format-2 metadata writes change only the
profile file. See [profile provider and safety limits](../configuration.md#native-machine-actions).
MCP Settings edits already use native config.save; config.action has no
MCP-management action. This does not finish every profile activation or the
remaining setup/keyring producers. Native service startup is Python-independent;
retained requests still start their provider on demand.

## Settings display compositor

`config_read_model::settings_view` renders the complete settings-envelope shape
from completed, sanitized management and catalog observations. It covers routing
policy/pins/legacy tiers, defaults and stored ceiling fields, profile sections,
MCP environment **presence only**, connection labels/status, revision tokens,
missing-file flags, catalog duplicate-label disambiguation, exact context/effort
observations, setup status and weekly plan usage. Missing quota observations
stay null; Codex windows count as weekly only with a 10080-minute duration.
The highest reported weekly usage wins; equal usage keeps the first sorted
connection's reset time.

This is a pure in-process library seam, not an advertised socket operation and
not a file loader, route authorizer, refresh producer or Python fallback.
All five management envelopes must be supplied; missing/malformed roots refuse
rather than returning invented empty settings. Failed envelopes retain the
management reader's first-error order. Catalog/setup refusals remain empty
observations as in the retained compositor. Input observations must already be
validated and credential-redacted by their trusted producers. UTC source
timestamps (including numeric offsets) are supported; arbitrary legacy Python
ISO parsing and local-timezone interpretation are not claimed.

Native display now has adapter/setup and Workspace-default producers through the
further composed paths below. They do not replace launch-time install trust. The catalog-composed reader below now loads the full
cached catalog/selector and computes display eligibility. The connection/MCP/routing management and advisory readers
below are now connected to the compositor. Connection revalidation now has the
separate host-only filesystem helper and explicit host dispatch below.
Native keyring refresh has the explicit unsupported-result contract below. Any explicit usage refresh must honor the
side-effect decision above (no lazy Claude CLI from display reads); no refresh
producer is introduced by the read-only composition. The remaining producers are
not replaced by supplying an already-composed observation to the library.
Only after those boundaries and the complete machine response are tested can
`config.read` leave the retained Python port.

## Composed connection and advisory readers

`ConfigurationReader::connection_management` loads one bounded private registry
snapshot and returns the retained connection-management shape: exact byte revision
(including whitespace), exists flag, sorted records, credential kind/owned metadata,
engine support/version policy and operations. It never returns credential paths,
values or keyring identifiers. An existing empty file has its own SHA-256 revision;
missing files use `absent`. Malformed/unsafe files refuse without repair.

`ConfigurationReader::settings_with_readers` connects that loader, Claude quota
cache reads, bounded Codex rollout observations and exact-model context caches
to `settings_view`. It now also loads the MCP management envelope described below.
Its trusted embedding still supplies the other three management
envelopes, workspace, catalog/selector entries, routes and setup discovery.
Entries must bind to a connection in that same registry snapshot before cache IO.
It returns the unchanged settings shape plus separate, sanitized advisory
diagnostics. This library method has **no production dispatcher caller**.

Codex native references must equal either the expected client `HOME/.codex`
or the id-derived owned state directory, with no symlinked ancestors, private
owner metadata and no duplicate reference. These observation checks are not
credential validation or route approval. The rollout observer skips symlinked
directories/files, inspects at most two newest days and ten newest files and
reads at most a 256 KiB tail per file. Missing/expired/unusable observations remain
unknown; they never fall back to a stale Codex quota cache: a cached Codex window
is used only when it records a newer free account read than the rollout. Claude
reads only its connection cache; the reader itself runs no CLI and takes no quota
lock, probe or write (the separate background refresh does). Malformed/unreadable private quota/context reads
produce separate diagnostics and unknown values. Non-object/invalid individual
cache values and absent caches also remain unknown. No cache failure authorizes
a route or reports zero use. Reads are not a multi-file transaction or protection
against hostile same-user filesystem replacement.

This completes the connection metadata and advisory read composition, **not**
connection revalidation, explicit refresh producers, adapter/setup discovery,
catalog eligibility, the other three management producers or machine dispatch. Active
`config.read` and its retained effects are unchanged.

## MCP management reader

`ConfigurationReader::mcp_management` returns the existing MCP management shape
from one private bounded descriptor read, including the exact byte revision,
missing-file defaults, `servers`/`mcpServers` precedence, supported/editable keys,
insertion-ordered server entries and per-entry validation problems. Object env
names keep insertion order; duplicate JSON keys retain their first position and
last value as in Python. Env values become presence flags only, including invalid
or disabled entries. Unsupported keys are listed, not copied.

Validation is display-only: commands are not resolved, declared tool/path/network/
role/engine policy is not installed or granted, and declared paths are not opened.
Disabled entries are checked as if enabled, preserving their repair guidance.
The reader neither runs an MCP server nor changes the executable MCP loader.

`mcp_management_observation` returns the same management success/error envelope
used by the compositor. Malformed JSON, non-object documents and missing/invalid
server maps preserve safe, value-free surface errors; the settings composition
keeps the other supplied surfaces instead of inventing a successful empty MCP view.
Unsafe storage still refuses the composed library read. The lower-level
`mcp_management` method refuses malformed documents with `invalid_mcp_registry`.
Private-file rules intentionally refuse more than permissive legacy inputs;
nonfinite JSON, excessive nesting and unhashable runtime-env entries do not
reproduce Python exceptions or unsafe parser behavior.

`settings_with_readers` now replaces its supplied MCP envelope with this actual
native observation. It still needs trusted completed user-default, routing and
profile envelopes, workspace defaults, catalogs/selector entries and setup.
No production caller or machine dispatch has been switched.

## Routing management reader

`ConfigurationReader::routing_management` loads the complete routing management
surface from explicit host roots. Its optional project pair contains the logical
project root and the host-resolved channel routing file, so linked-worktree/channel
resolution stays with the host. It never obtains paths from client JSON or ambient
HOME/XDG. Project, explicit environment override and user sources retain their
individual exact-byte revisions, exists/valid flags and original rule spelling.
The first regular file wins even if malformed; lower rules never merge or repair it.
Only effective rules expand legacy implementation work kinds. Missing routing
remains missing, not a successful model-policy migration.

The reader reuses the native project-save reply's file/builtin/catalog observers.
Named version, missing-tier and ambiguous-request-label errors retain their
management shape and source-order first-error semantics. Other malformed JSON
and structural errors use bounded value-free messages instead of Python parser
text or saved values. Nonfinite/excessively nested JSON refuses; generic Python
parser equivalence is not claimed. Unsafe/oversized storage can refuse the read
rather than returning a permissive legacy view.

Cached efforts come from each connection's newest genuine snapshot, skipping
cross-connection placeholders but not a genuine empty/no-model result. Invalid
snapshots contribute no support; seed/missing evidence is not an observation.
Conflicting connection selectors stay null, an observed default-only selector
is empty, and default aliases follow the retained presentation rule. No installed
adapter, credential, keyring, process, network, lock, refresh or registry rewrite
is involved. Catalog directories must be private real directories before listing:
unsafe shared state/catalog roots refuse, while one connection's unsafe directory
is not listed and only withholds that connection's levels (Python
`_routing_model_efforts` skips a refused connection). Directory listings and
snapshot bytes are bounded. This is not the full route-catalog/selector
eligibility producer, adapter discovery, installation trust or dispatch authority.

`settings_with_routing_readers` replaces the supplied routing envelope with that
actual producer, then invokes the existing connection/MCP/advisory-composed
reader. The newer `settings_with_management_readers` below also replaces defaults/profile
envelopes. Workspace defaults, full route catalog/selector and setup remain trusted
supplied observations. The earlier `settings_with_readers` retains its
three-supplied-envelope seam. None has a production dispatcher caller. Later composed paths below add Workspace/setup discovery and selected-connection
refresh dispatch with an explicit unsupported keyring contract. Remaining refresh
producers and full machine-boundary tests still block the `config.read` flip.
The catalog-composed method below replaces the earlier supplied catalog/selector seam.

## Defaults and profile management readers

`user_defaults_management_observation` reads the exact defaults-file revision,
exists flag, editable/preserved keys, language choices and stored ceiling. It does
not validate away unknown saved defaults or rewrite them. Portable coordinator
resolution is display-only: the newest genuine per-connection cached catalog,
configured non-gateway state, launch presence, discovery provenance, exact enabled
non-denied identity and observed parameter pairs must yield a unique match.
Automatic-worker `providers` policy does not restrict this owner selection.
Top-level parameters override the portable parameters only after the portable
value itself validates. Missing/seed/unobserved, ambiguous and unsupported
parameter observations remain unavailable, never a minted route. As in Python
`read_user_defaults`, a connection snapshot refusal makes `coordinator_resolution`
unavailable with that reason code instead of refusing the defaults read; no
account/credential target is opened.

`profiles_management_observation` reads formats 1/2, converts format 1 only in
memory, and snapshots the four user backing files independently. It reports exact
file revisions, normalized section settings, sorted entries with canonical entry
hashes, summaries, matches, selected-profile differences (bounded to 200), missing
connection notes, applied/pending metadata and project/environment shadowing.
States retain their priority: interrupted, none, unavailable, modified, in-sync.
Disabled extra MCP servers do not create differences; missing saved connections
produce notes rather than a fabricated change. Unknown defaults are not captured;
MCP commands/args/env are excluded. A broken unselected section still contributes
a problem but does not make a valid selected section unavailable. An unreadable
section has a null revision and is never considered synchronized when selected.

All roots and the optional logical-project/channel-routing pair are trusted host
inputs. Reads never create/repair roots, migrate/save/activate a profile, lock,
probe an executable or open a project store. Private bounded-file rules intentionally
refuse more than permissive legacy inputs. Invalid profile metadata uses bounded
value-free message/field diagnostics; section-validation problems likewise omit
saved values. Malformed JSON keeps the existing safe management error. Nonfinite,
excessively nested and permissive malformed legacy ceiling values are not supported
as successful observations. Separate backing-file observations are not a transaction
and do not establish hostile same-user isolation.

`settings_with_management_readers` replaces all five management envelopes with
native file producers before invoking the existing settings compositor. Workspace,
full route catalog/selector and setup observations remain supplied. Earlier seams
remain available; no machine dispatch, writer or unknown-effect/single-owner fence
changes.

the recorded capture `crates/workspace-service/tests/fixtures/defaults-profiles-python.json` (its Python generator was removed)
reproduces 65 offline current-Python defaults/profile pairs and 64 complete settings
envelopes under disposable HOME/XDG, fake executable PATH and a process-execution
audit hook. The remaining malformed-parameter case retains the Python compositor's
`AttributeError` explicitly; it is not a passing Python settings comparison.
The Rust `defaults_profiles` tests compare successful and failed management
envelopes, declared diagnostic deltas, composition, revisions, unchanged storage
and unsafe-file/catalog refusal. No real engine is launched.

## Complete cached catalog/selector reader

`ConfigurationReader::catalog_selector` produces the full route-catalog surface
and selector entry observations from the real registry, effective routing and
per-connection cached snapshots. It uses no seed fallback. Newer cross-connection
placeholders are skipped; a genuine empty/no-model observation stops fallback.
Catalog records follow engine/name/id order; raw selector observations use the
same deterministic connection order, with each snapshot's entry order preserved.
Snapshot filename/document/entry identities, engine/connection bindings, route
hashes and evidence shapes must agree. A route hash is Python's
`route_fingerprint` over the stored pairs sorted but never deduplicated:
claude-agent-acp advertises its `fast` and `ultracode` options as two
`model_config` on/off selects, so a real Claude route repeats those pairs. The
native catalog producer stores the same identity. Catalog enumeration and file reads retain
the existing bounds and private, no-follow rules. Equal snapshot mtimes refuse
rather than guessing an authoritative newest identity from directory order.

Model presentation preserves engine names, derives readable versioned labels only
from exact IDs, and removes duplicate/unresolved default aliases. Display policy
covers exact enabled/deny/provider lists, automatic membership, configured tiers,
all seven optional policy attributes and their sources, work-kind expansion,
per-source matches, unmatched enabled/automatic identities, timestamps and reason
precedence. No family/order supersession is inferred. Raw selectable observations
supply route/connection identities, names and exact non-default thought-level
values for the existing settings compositor and context-cache reader.

**Read-effect decision:** selector display uses stored connection state and
evidence, not the retained selector's incidental credential revalidation/write.
Gateway and unconfigured records remain unselectable; authorization-required
evidence needs configured promotion. Provider restrictions affect automatic
eligibility, not owner selectability. Qualification metadata is reported, not
invented or treated as dispatch approval. Secret/keyring references are metadata
only: no credential targets or keyring APIs are opened.

`settings_with_catalog_readers` replaces catalog/selector observations and all
five management envelopes before invoking the existing compositor. Like retained
machine `config.read`, its catalog uses user/environment policy even when a
project-specific management view is requested. Workspace, setup discovery and
any project-ceiling observation remain trusted supplied inputs. As in Python
`read_route_catalog`, a connection whose newest snapshot is malformed, unbound,
unsafe or ambiguous contributes no routes and one `catalog_problems` row
(`connection_id`, reason `code`, owner-facing `message`); the connection stays
listed, other connections keep their models, and that engine's settings
catalogue reports `failed: true`. The file is never rewritten or removed. Unsafe
shared roots and registry/routing failures still refuse the whole read.
For compatibility, the catalog's `observed` display flag means no seed fact is
present; empty evidence is not proof of discovery, authentication or qualification.
It never adds a dispatcher, refresh caller, Python replay, store or scheduler.

the recorded capture `crates/workspace-service/tests/fixtures/catalog-selector-python.json` (its Python generator was removed)
reproduces 68 complete current-Python catalog/selector and settings pairs under
disposable short HOME/XDG roots, fake executable PATH and a process-execution audit
hook. The real Python snapshot loader, selector, catalog policy and compositor
run; only selector validation is replaced by stored records per the explicit
read-effect contract, Claude lazy refresh is suppressed and setup remains
unavailable. Project-ceiling observations remain supplied. These are not live
credential validation or end-to-end dispatched `config.read` captures.
The Rust `catalog_selector` tests compare every field, check source hashes,
replace forged supplied catalog/selector inputs, verify unchanged storage and
degrade only the connection with an unsafe/unbound/ambiguous catalog.
`catalog_route_identity` reads a real-shaped Claude snapshot whose route id comes
from the Python oracle and binds it as coordinator and native route. Private-file/identity bounds
are deliberately stricter than permissive legacy reads. Finite captures do not
prove arbitrary parser/Unicode/legacy-value equivalence or release readiness.

The Workspace/setup and host dispatch paths below complete passive composition
and selected filesystem refresh, with an explicit unsupported keyring contract.
The machine boundary now uses passive native Display; explicit catalog/setup
refresh uses the retained Python config.action provider before any native action effects.
Native quota refresh is unavailable. Project-ceiling editing remains in project.config.read/save rather
than machine-global settings. Launch-time adapter/account trust is not replaced
by display discovery.

## Workspace defaults composition

`ConfigurationReader::workspace_defaults` reads only mode/checkout from
`<workspace-channel-state>/defaults.json`. The embedding supplies that absolute
trusted host path separately from XDG engine state and optional project routing;
it is not a model/socket argument. Existing ancestors must be real directories,
the existing channel root owner-private, and the bounded (4 MiB) regular file
owner-private, single-linked and opened no-follow/nonblocking. Missing roots/files
return an empty observation without initialization. Malformed JSON, non-object
documents and unsafe storage refuse; diagnostics contain no saved values.
Unknown keys are not returned. The existing compositor supplies guarded/main
only for absent keys, preserving explicit nulls and legacy display values. None
of these observations grants execution permission.

`settings_with_workspace_readers` replaces any supplied Workspace observation,
then invokes the existing cached-catalog and management composition. Both current
defaults and saved-profile defaults receive the same Workspace mode/checkout.
Workspace storage refusal returns no partial settings; one connection's catalog
refusal is that connection's catalog problem (see above); existing per-surface
management errors and advisory diagnostics remain intact. This lower-level seam
still accepts setup; the setup-composed path below replaces it. An optional
project-ceiling management observation is still supplied when applicable.
There is no project database open, writer, refresh, replay or second owner.
Separate reads are not a multi-file snapshot or hostile same-user isolation.

the recorded capture `crates/workspace-service/tests/fixtures/workspace-composition-python.json` (its Python generator was removed)
reproduces 34 real offline Python cases: 28 complete Workspace/settings pairs and
six retained errors (two JSON refusals and four non-object Python exceptions).
The native implementation returns bounded refusals instead of Python attribute
errors. Captures use disposable short HOME/XDG roots, fake executables, and a
process-execution audit hook; no engine runs. Real management/catalog/compositor
code runs with stored selector state and suppressed lazy Claude refresh per the
read-effect contract; setup is unavailable and no project/store is supplied.
Cases cover missing/empty state, mode/checkout independently, null/empty/false and
legacy display values, Unicode, duplicate keys and ignored keys, with and without
saved profile defaults and genuine three-engine cached catalogs. Four Rust tests
compare every response field, retain source hashes/advisory diagnostics, prove
nonmutation and channel-root selection, and cover file/root/link/FIFO/size/encoding
refusals plus failure propagation from other producers. Finite fixtures do not
establish arbitrary JSON parser equivalence or end-to-end machine dispatch.

## Passive adapter/setup composition

`ConfigurationReader::setup_discovery` accepts explicit trusted `SetupEnvironment`
data/PATH/node-override and optional current-channel bundled-engines roots.
It never reads process-global environment or searches client-managed engine trees.
Bundle candidates require an executable node, a qualified declared adapter version,
an in-package bin, canonical trusted files and bounded stable readable adapter
bytes. Codex bundles also require their nested/hoisted Codex package and platform
binary. The manifest is rehashed against the selected bytes. Missing/incomplete
bundles fall back to owned npm trees; unsafe storage refuses instead. This is
not an approved byte pin, a launch command, instruction delivery or account check.

Owned selection prefers qualified **declared** versions, then numeric versions
and side-by-side SDK directory versions; equal ranks keep the first sorted path.
Ambiguous/malformed manifests, missing/escaping bins and missing node are absent
candidates. Manifests are bounded at 4 MiB, stable bundle files at 512 MiB each,
and owned-root enumeration at 512 entries. Symlinked package parents/roots and
unsafe status/lock storage refuse without repair. Node resolution may canonicalize
a host-supplied override or the first executable node on explicit PATH. All roots
must be absolute without parent traversal; data storage retains the project fence.

Discovery reports login-marker **metadata only**, sorted existing connection
precedence, registry revision and engine opt-outs. It reads allowlisted automatic
setup phases and safe catalog-failure reason codes. Busy phases nonblockingly
probe an **existing** private cooperative flock: held means busy, missing/free
means interrupted. The descriptor closes immediately; no lock/root/file is created.
Malformed phase JSON is `setup_failed`; unsafe files refuse the whole composition.
The legacy settings `automatic` field retains Python's phase-string truthiness;
it is not a setup success signal.

OpenCode's version producer would require executing a binary. It is deliberately
not run: its version remains null and qualification is not asserted, a stored
record still displays connected, and otherwise status is `discovery_unavailable`. A separate retained
`version_probe_disabled` diagnostic always records that gap. No absent-install or
qualified-version result is fabricated. Explicit contained discovery remains
outside display reads; no OpenCode setup/connection is automatically created.

`settings_with_setup_readers` replaces supplied discovery, then invokes the
Workspace/catalog/management compositor and retains all diagnostics. Unsafe
discovery returns no partial settings. No project database, scheduler/owner,
writer, onboarding, download, quota refresh or Python fallback is introduced.
Separate reads/lock observations are not a transaction or hostile same-user isolation.

the recorded capture `crates/workspace-service/tests/fixtures/setup-discovery-python.json` (its Python generator was removed)
reproduces 54 current-Python discovery/settings pairs with disposable short
HOME/XDG roots, fake executables and a process-execution audit hook. Real bundle
resolution/verification, management, discovery and settings code run; selector
validation uses stored records and lazy Claude refresh is suppressed. OpenCode's
probe is disabled by the explicit read-effect decision, and that status delta is
asserted separately, not counted as exact unmodified Python parity. Four native
tests compare every other field, source hashes and diagnostics, and exercise
missing state, forged supplied inputs, busy/interrupted locks, unsafe files/roots,
nonmutation and propagation of upstream refusals.

Machine `config.read` now uses this native setup composition through passive
host dispatch. Selected-connection refresh and the unsupported keyring-result
contract remain host-only. Explicit catalog/setup producers still use Python;
native lazy usage refresh and launch-time trust replacement are not supplied here.
The service-boundary comparison and failure checks are described below.

## Explicit filesystem registry refresh

`config_refresh::refresh_connections` is a separate trusted-host library action,
not a method on the read-only reader and not an advertised machine/socket/model
operation. No production dispatcher calls it yet. Display reads never call it.
The embedding supplies explicit trusted roots, selected connection IDs, an exact
read revision and its UTC observation timestamp. Existing private roots and an
existing valid registry are required; missing state is not seeded or repaired.

It acquires the same `connections.lock` flock as the retained Python transaction,
then reads/rechecks the exact byte revision and preflights every selected record.
Unknown IDs, changed approved fingerprints and selected keyrings refuse the
whole batch before credential-target IO. Keyring refresh is explicitly
unsupported; no keyring success/absence is invented. It cannot register a
connection, approve a fingerprint, install/validate an adapter or validate an account.

For filesystem references it derives the existing canonical owned/client-native
directory or secret path and checks collisions before metadata IO. Referenced
client-native directories may intentionally share the same engine path, matching
the retained registry rule. Native markers and secret files are checked only for
metadata; their bytes are never opened. Secret emptiness remains authorization
required, not configured. Invalid paths/modes/types remain invalid.

One lock covers the entire selected batch's revalidation and replacement.
Only changed states and their timestamps are modified; names, session limits,
qualification, references, approval, override history and opt-outs are preserved.
No-op refresh keeps the exact original bytes/revision. A changed registry is
bounded, written to a private exclusive temporary file, fsynced, revision-rechecked,
atomically renamed and directory-fsynced. Pre-commit failures leave the registry
unchanged; a failure after rename returns `machine_outcome_unknown`, never a
Python retry. The caller must re-read before deciding another action.

The helper can create the private shared lock as an explicit-refresh effect, but
never creates/repairs roots, changes permissions, writes ignore files, launches
an executable, probes quota, or opens a project store/scheduler. It is cooperative
same-user arbitration, not protection against hostile same-user filesystem races.
Unlike the permissive retained validator, changed fingerprint approval refuses;
unrelated/legacy-omitted fields are preserved rather than normalized on rewrite.
The helper is not invoked by machine display and is not complete setup/refresh migration.

the recorded capture `crates/workspace-service/tests/fixtures/registry-refresh-python.json` (its Python generator was removed)
captures 62 real Python filesystem-validation transactions under disposable
HOME/XDG with an empty executable PATH and process execution forbidden.
`--check` reproduces the capture. The Rust `registry_refresh` tests compare states,
changes and parsed registry results, verify unchanged credential storage, no-op
bytes, concurrent stale revisions, unsafe files/locks, unsupported selections and
non-repair. An internal failure-injection test checks pre-commit cleanup and
post-rename unknown-effect fencing. These are explicit library-action tests, not
end-to-end machine settings or installed-client qualification.

## Native config.read host and machine dispatch

`config_read_dispatch::ConfigReadHost::read` composes the setup/Workspace/catalog/
management readers for a typed `Display` or `RefreshConnections` host intent.
The active machine/socket dispatcher selects only `Display`; refresh remains a
typed host-only API, never inferred from socket JSON. Trusted host roots, optional
resolved project routing and sanitized project observations retain their boundaries.
The machine read is global (as in Python); project overrides and ceilings use the
separate project.config.read operation. Dev uses channel-local config/state/data,
without reading release settings. Release uses HOME/XDG roots. Bundle selection
is restricted to the installation containing the configured machine tools, with
the compiled channel package identity, never another channel or a socket path.

The result also carries `catalogueRefresh: {running, failed: [{connectionId, reason}]}`,
outside the frozen settings envelope: `machine::config_read::read` first plans the
background model observation (`config_catalog_startup`) for every configured,
launchable connection with no saved snapshot or only one older than 24 hours, so
the reply that starts it already says `running`. `failed` lists only connections
that still have no saved list. Reads never wait for the observation. Each
connection is observed on its own thread; their shared private folders are
created race-safely (a folder another thread or process created first gets the
same ownership/mode checks, never an `AlreadyExists` failure). A connection
observed well is not started again for 10 minutes; after a failure a later read
starts it again 5 seconds later, the wait doubling with each further failure up
to those 10 minutes. The Workspace app keeps reading the settings while a list is
`running` (every 1.5 seconds) and while one has `failed` (every 10 seconds,
doubling to 5 minutes), whether the read came from the store or the port's
machine poll, so a list that arrives later reaches the composer without a relaunch.

The existing result fields are preserved, with additive `result.diagnostics` for
usage/context/setup/keyring unknowns. Refusals contain only a coarse code and safe
message, without partial settings. Native reads use the bounded settings lane and
have no deadline: a slow read is waited for ([slow reads](../cortex-workspace/consistency-and-recovery.md#slow-reads-are-waited-for));
a disconnect does not trigger Python fallback. The service
may still append its ordinary sanitized helper audit event outside settings storage.

Display never calls refresh or initializes state. Refresh first completes the
display preflight, then passes the exact supplied registry revision, explicit IDs,
trusted UTC timestamp and bounded lock wait to `refresh_connections`. The helper's
whole-batch keyring/ID/fingerprint preflight, arbitration and atomic persistence
are unchanged. A successful refresh is followed by a fresh composition whose
connection token must match its receipt. A failed or superseded post-refresh
projection returns `machine_outcome_unknown` (also conservatively after a no-op);
inspect before another action. There is no fallback, automatic retry, scheduler,
project store or owner acquisition. Other files are independent observations, not
a cross-file transaction. Library refusals return no partial result; existing
per-surface unavailable fields and advisory diagnostics retain their semantics.

**Keyring decision:** the current native reader has no qualified OS-keyring
backend and never contacts one or reads a secret. Display preserves the stored
connection state and adds a value-free diagnostic containing only connection ID,
source `keyring` and code `keyring_not_observed`. This is not a fresh presence,
absence, unlock or authentication result. Selecting any keyring for refresh
refuses the entire batch with `keyring_refresh_unsupported`; it never skips a
record, converts it to filesystem storage, persists unavailable/authorization state
or falls back to Python. Filesystem-only selections remain usable with unselected
keyrings present. This explicit unsupported-result contract is a deliberate
capability difference, not Python parity or removal of keyring launch support.
Production dispatch validation remains unchanged.

the recorded capture `crates/workspace-service/tests/fixtures/config-read-dispatch-python.json` (its Python generator was removed)
reproduces 14 real offline Python validation/settings captures: four filesystem
cases and ten fake-keyring cases across Codex/Claude (present, empty, absent,
backend error and unavailable module). The empty fake value is present in Python;
native refusal is asserted separately, not counted as equality. Captures use
disposable HOME/XDG, fake executable PATH and a process-rejecting audit hook, never
the owner's keyring/config or an engine. Stored-state display and disabled lazy
usage/OpenCode probes follow the table above. Comparison adjusts only the declared
OpenCode status delta and connection byte tokens (different temporary roots/JSON
serializers); native tokens are independently hashed from actual bytes. All
other settings fields and parsed refreshed registries compare directly.

The `config_read_dispatch` integration tests cover display nonmutation, successful
and no-op refresh, keyring diagnostics/refusal, mixed-batch atomic refusal in both
orders, filesystem selection with an unselected keyring, stale revisions, invalid/
unknown IDs, missing-state noncreation, sanitized results and preflight failures
before lock creation. The internal dispatch test injects failed/superseded
post-refresh projections; the existing helper tests retain the real persistence
failure/lock/concurrency coverage. These are host-library checks, not installed-client, keyring-backend or live qualification.

`cargo test --locked --offline -p workspace-service --test config_read_service`
starts the real service binary and calls machine.call over its private socket.
Fourteen full replies compare against the offline Python captures above, adjusting
only the declared OpenCode status, independently hashed connection byte tokens and
additive diagnostics (asserted separately in full). Disposable HOME/XDG, fake
executables and an execution sentinel prevent reliance on the owner's config or
real engines. It also covers first-run missing-state noncreation, malformed-state
safe refusal without partial results, ignored refresh/root JSON, stored keyring
diagnostic delivery, byte/mode nonmutation and explicit service shutdown. Socket
roots remain short independently of TMPDIR. This is no-paid service-boundary
evidence, not installed desktop/Paseo/Zed or full side-effect parity.

## Offline checks

the recorded capture `crates/workspace-service/tests/fixtures/config-read-python.json` (its Python generator was removed) captures
current Python under disposable HOME/XDG roots in a short temporary directory.
Only a fake Claude executable is reachable. It verifies that the retained quota
reader really creates its lock/probe and invokes that fake; no real engine,
account, network or paid call is used. `--check` regenerates and compares the
fixture without changing it. The routing corpus supplies inputs only; profile
expected results are independently captured from Python.

`cargo test --locked --offline -p workspace-service --test config_read_model`
compares the captured library outputs, verifies source hashes, and checks that
native filesystem reads leave bytes/modes/directory entries unchanged.
Symlinks, unsafe permissions, directories, relative/project roots and invalid
connection IDs refuse. Library tests are not installed-client qualification
or evidence that Python can be removed.

the recorded capture `crates/workspace-service/tests/fixtures/settings-view-python.json` (its Python generator was removed)
captures 56 complete Python settings envelopes with disposable HOME/XDG roots,
an empty executable PATH and no engine calls. Real missing-file management
reads supply the base surfaces; controlled observation inputs exercise the
display-projection seam, not the unported producers. The Rust `settings_view`
test compares every envelope, checks current source hashes and refuses missing
composition roots. `--check` proves fixture reproducibility. These are library
comparison tests, not end-to-end `config.read` or installed-client evidence.

the recorded capture `crates/workspace-service/tests/fixtures/config-readers-python.json` (its Python generator was removed)
captures 30 real Python connection-management reads and four complete settings
envelopes with actual cache/rollout/context reads under disposable HOME/XDG.
The retained lazy CLI function is explicitly suppressed per the native contract;
a no-execution audit hook rejects process attempts. Unported management/catalog/
selector/setup inputs remain controlled seams, not qualified producers.
Path-dependent connection revisions are independently checked against Python's
exact raw bytes before normalization and rebound to the installed disposable
fixture bytes during replay. All other expected fields are captured unchanged.
`--check` verifies reproducibility.
`cargo test --locked --offline -p workspace-service --test config_readers`
checks parity, nonmutation, byte revisions, unsafe-file refusal, canonical/owned
native directories, ambiguous/symlinked references, cache failure/unknown behavior
and rollout expiry/day/tail bounds. These checks do not qualify machine dispatch,
installed clients, account validity or any refresh effect.

the recorded capture `crates/workspace-service/tests/fixtures/mcp-management-python.json` (its Python generator was removed)
captures 67 current-Python MCP management envelopes and 67 complete settings
envelopes under disposable HOME/XDG roots with process execution forbidden.
MCP reads, validation, redaction and display projection are real; the other
management/catalog/setup inputs remain controlled seams. Empty-registry plan
usage was read through the former Python implementation when the capture was recorded, not synthesized.
`cargo test --locked --offline -p workspace-service --test mcp_management`
checks those comparisons, provenance, file nonmutation, unsafe storage refusal,
secret-free diagnostics and replacement of supplied MCP observations.
These are library checks, not end-to-end dispatch or installed-client evidence.

the recorded capture `crates/workspace-service/tests/fixtures/routing-management-python.json` (its Python generator was removed)
captures 31 routing views, 31 complete settings envelopes and 20 cached-catalog
cases with disposable HOME/XDG and process execution forbidden. Complete Python
results are retained; the explicit structural-error message deltas have separate
native expectations and an assertion that no other field differs. Other
management/catalog/setup inputs in complete settings captures are controlled
seams, not native producer evidence. `--check` proves reproducibility.
`cargo test --locked --offline -p workspace-service --test routing_management`
checks parity, provenance, byte/mode/directory nonmutation, channel-specific paths,
malformed/unsafe catalog unknowns and unsafe/oversized routing refusal. These are
library checks, not installed-client or end-to-end dispatch qualification.

See [configuration](../configuration.md), [architecture](architecture.md) and
[verification](verification.md).
