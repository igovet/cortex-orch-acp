# Cortex configuration

> **Current contract:** automatic model identity, tier, effort, request labels and eligibility are owner data in routing version 2. Every model name and routing JSON example in the older material below is a legacy illustration, not a current setup default; its built-in/family inference rules are removed. See [settings-only model policy](#settings-only-model-policy).

This reference describes configuration the current ACP runtime actually reads. The source of truth is the loaders named in each section. Private files are local operator state; do not commit credentials or real connection registries.

## Where configuration lives

The paths below are the default/release layout. For a Rust Paseo launch, build
`cortex-acp` and `cortex-paseo-tools` for the same channel (the checkout build
default is dev). Settings, automatic setup and the agent resolve raw HOME/XDG
roots through the same compiled-channel rule. Dev uses
`$XDG_CONFIG_HOME/cortex-workspace-dev/cortex/`,
`$XDG_STATE_HOME/cortex-workspace-dev/state/cortex/`, and project `.cortex-dev`;
missing XDG values first default to `~/.config` and `~/.local/state`. Release
uses the table's ordinary paths. `CORTEX_WORKSPACE_CHANNEL` at process launch
does not override a Rust binary's compiled channel. An empty `CORTEX_ROUTING`
is treated as unset. Outside extracted Dev, a valid absolute override still
takes precedence over user routing. Native-account HOME is unchanged. Settings displays the
resolved binding reported by `cortex-paseo-tools config --host`, not an inferred
release path. Existing configuration is not copied or migrated across channels.

| Data | Default path | Scope and precedence |
|---|---|---|
| Connection registry | `$XDG_CONFIG_HOME/cortex/connections.json` or `~/.config/cortex/connections.json` | User-wide. Managed by `scripts/cortex-paseo-tools.sh` setup commands (`add-connection`, `add-opencode-connection`, `install-engine`), by Settings > Connections (native-account connections only) and by the startup sync for an engine that has no connection; it has no project override. |
| Connection secrets | `$XDG_CONFIG_HOME/cortex/secrets/<connection-id>` | User-wide private files, written by setup. Native-account credentials can instead be referenced from the existing user engine directory. |
| User defaults and policy ceiling | `$XDG_CONFIG_HOME/cortex/defaults.json` | User-wide base configuration. |
| Routing rules | `<project>/.cortex/routing.json`, then `$CORTEX_ROUTING`, then `$XDG_CONFIG_HOME/cortex/routing.json` (or `~/.config/cortex/routing.json`) | First existing file wins; files are not merged. The bundled default preset is written only when the file is absent: by setup, at each agent start and by Settings' Apply action. Existing owner files are never overwritten. See [Default preset](#default-preset). |
| User routing profiles | `$XDG_CONFIG_HOME/cortex/routing-profiles.json` | Saved owner settings: routing, defaults/ceiling, MCP usage policy and connection limits. Activation materializes these user files; project settings are not captured. |
| Cortex-owned external MCP servers | `$XDG_CONFIG_HOME/cortex/mcp-servers.json` | User-wide only. Project-local MCP registries are intentionally ignored. |
| Engine-native state and catalog snapshots | `$XDG_STATE_HOME/cortex/connections/<id>/...` or `~/.local/state/cortex/...` | User-wide mutable state, separate from configuration. |
| Paseo launch record | `$XDG_CONFIG_HOME/cortex/paseo-plugin-launch.json` or `~/.config/cortex/paseo-plugin-launch.json` | Owner-private non-secret command, environment and shim metadata written by the setup helper in the environment it describes. A Paseo instance with its own `XDG_CONFIG_HOME` (such as the isolated dev daemon) has its own record and does not use another instance's `~/.config` record. |
| ssh password dialog (optional, off) | `$XDG_CONFIG_HOME/cortex/ssh-askpass.json` | User-wide, written by hand. See [ssh from workers](#ssh-from-workers). |
| Task and report store | `<project>/.cortex/cortex.sqlite3` and sibling published Markdown | Project data, not operator configuration. |

Workspace's `project.config.read` editor now reads project routing and the saved project ceiling in Rust, with unchanged editor fields and revision tokens. It never substitutes user/environment routing for an absent project override or initializes a missing store. Pre-save MCP discovery (`config.mcp.discover`) now lists tools through a contained Rust subprocess without saving a server or invoking a tool. Project saves (`project.config.save`) now also run in Rust, retaining revision conflicts, aggregated validation, routing bytes and project-default-preserving ceiling transactions. Machine-wide saves and the selected metadata/profile/reset actions now also run in Rust; unqualified activation/preset shapes and unported setup/catalog/keyring producers retain Python; see the [native machine boundary](cortex-workspace/local-task-execution.md#native-machine-boundary).

The additive Rust [configuration observation foundation](project/native-config-read.md)
parses profile formats 1/2 and connection snapshots, route fingerprints and
advisory adapter/login/usage/context markers. Its composed reader now loads
credential-redacted connection management metadata with exact byte revisions,
Claude quota caches, bounded Codex rollouts and exact-model context, feeding the
Workspace settings compositor without incidental writes or probes. MCP management
is also loaded natively: exact byte revisions, ordered entries, per-entry problems
and env presence only, with safe per-surface errors for malformed documents. The
additional routing-composed path loads project/environment/user precedence,
byte revisions, normalized rules and cached effort observations. It does not merge
files or fall through an invalid earlier file. Its management-composed path now
also reads defaults/ceilings and profiles with exact byte revisions, in-memory format-1
conversion, cached coordinator binding, snapshot differences and unavailable/interrupted
states. It never activates or rewrites a profile. Its catalog-composed path now
loads the full cached route catalog/selector, including genuine snapshot fallback,
default-alias presentation, exact owner-policy diagnostics and observed efforts.
Stored connection states are display observations, not authorization or live
validation; malformed/unsafe/ambiguous catalogs refuse the whole composition.
The Workspace-composed path also loads mode/checkout from the explicit host-selected
Workspace channel state root, distinct from user defaults and XDG engine state.
Absent keys display guarded/main; explicit null or legacy values remain display
values, not authorization. Missing state is not created, unknown keys are not
returned, and unsafe/malformed storage refuses without partial settings. The further
setup-composed path now supplies native prompt-free bundle/owned-adapter discovery,
login-marker metadata, opt-outs and passive first-run diagnostics. It never launches
an executable. Unprobed OpenCode discovery stays explicitly unavailable (or connected
when a stored record exists), with a retained diagnostic and no invented version.
Machine `config.read` now uses the native passive display compositor, with
sanitized `diagnostics` added to its result. Socket parameters never select roots
or trigger refresh, and a refusal never falls back to Python. Explicit
`config.action` uses the [retained action boundary](#native-machine-actions);
`config.save` uses the [native machine writer](#native-machine-saves). The native reader has no lazy Claude quota producer,
so missing/expired usage stays unknown.
The host-only dispatch accepts a typed display or explicit selected-connection
refresh intent; the machine socket selects Display only.
It preflights settings before effects, invokes the filesystem helper and re-reads.
A failed/superseded post-refresh projection is unknown and must not be replayed.
Native keyring refresh is explicitly unsupported: selected keyrings refuse the
whole batch; display preserves stored state with a `keyring_not_observed` diagnostic,
never a fresh authentication claim or a backend/keyring call. The separate host-only Rust `config_refresh::refresh_connections` helper now provides revision-locked,
atomic revalidation for selected filesystem-backed connections. Display reads
do not call it, and keyring refresh remains unsupported. It does not perform
setup, approve a fingerprint or verify an account.

### Retained engine connection setup

Machine `engine.connection` is native Rust for every request shape: `login_command`,
`recheck` and `add_key` run as below, and any other action is refused natively:

| Action | Current behavior |
| --- | --- |
| `login_command` | Native Rust: discover a qualified channel-local bundled/owned adapter, verify its install and return its native login executable/argv; never execute the login command. |
| `recheck` | Native Rust: run host-selected native status with discarded output and an eight-second timeout, then revision-lock create/reuse of a reference to the existing native account. No account credential contents are read or copied. |
| `add_key` | Rust: validate name/key/revision, verify the host-selected install, create the connection, privately save its key and validate metadata. Known post-creation failures remove the new secret/record and persist the engine opt-out; uncertain writes never roll back by guess or replay. |
| Unknown/missing action | Native Rust: `unsupported_action` ("Unsupported connection action."), no lookup or file effect. Truthy non-object params answer the ordinary `machine_tool_failed` RPC error, as the former Python helper's uncaught `AttributeError` did. |

The passive Rust setup display is not a replacement for install verification,
status checks or credential persistence. Login lookup uses a separate native
handler on the bounded inspection lane, not a passive display stub. Native
lookup/recheck/key-creation errors never retry through another implementation,
and no request starts a helper process: the former Python helper was removed. The native path answers the intended
refusal envelope for every refusal.
Recheck uses the retained filtered child environment and original HOME; timeout or
cancellation kills the direct status child. Registry files/lock/ignore remain 0600
and the managed root 0700. Reuse does not rewrite an existing reference or rederive
its state. API-key creation uses three separate shared-lock transactions (create,
secret write/state update, metadata validation), not one multi-file transaction.
Only the id-derived `$XDG_CONFIG_HOME/cortex/secrets/<connection-id>` is written,
with a private 0700 parent and atomic 0600 replacement; symlink/hardlink/foreign/
public secret targets refuse. Key bytes never enter the registry, replies, logs
or process argv. Existing OS-keyring references are not converted into files.
Failed cleanup or post-replacement durability is explicitly unknown; inspect
settings before retrying. These are cooperative same-user filesystem fences,
not protection against hostile concurrent same-user changes.
See [native lookup limits](project/native-config-read.md#current-boundary). `config.action` installers, catalog refresh, presets and profile
operations retain their separately documented providers below.

The offline `engine_connection_service` regression retains the 64-case Python
capture and compares complete replies/files/modes with zero helper calls for the
three native actions. Two provider-only injected exceptions now compare their
unchanged file effects and the real successful action reply: that artificial
Python hook is not a native failure signal. Native fault tests separately prove
unknown writes do not clean up or replay. A further immutable 36-case capture
covers Unicode boundaries, install failures, private-write/validation failures and
fake-keyring validation. Eight injected failure cases compare complete replies,
configuration effects and write sequences in native units; the other 28 compare
at the socket. Valid keyring-reference cases retain their Python provider and compare exact fake
lookup sequences (including absent, empty, present and unavailable cases);
stale-revision refusals are native and never look up the backend; malformed keyring identifiers refuse natively before any lookup. Only
validated generated IDs/timestamps and trusted host paths are rebound;
trust fingerprints are checked before rebinding. No real keyring, account, engine
download or installed application is qualified by these offline checks.

### Native machine actions

Machine `config.action` selects Rust for connection rename, session-limit edits
and confirmed default-preset reset, plus profile read/create/save/update/delete/
deactivate, qualified four-section activation/preset apply, filesystem-backed
connection validation and connection removal
on the bounded action lane. Other variants retain Python. Provider selection
happens before native admission, never after a failed or partially executed native
action. Unported working workflows keep their existing provider; a changed credential
shape after native admission refuses without replay.

| Action | Current provider and preserved behavior |
| --- | --- |
| `connection_update` / `rename` | Rust; exact-revision metadata edit; 1–120 printable characters, retaining whitespace. |
| `connection_update` / `set_max_concurrent_sessions` | Rust; exact-revision metadata edit; integer 1–64, never a boolean. |
| `connection_update` / `validate` | Rust for filesystem references: canonical/collision checks and metadata-only state derivation; keyring-backed validation retains Python for valid requests. Stale-revision refusals are native and never look up the backend; an unavailable provider refuses without changing credential state. No fingerprint approval or credential-byte read. |
| `connection_update` / `remove` | Rust: registry/history deletion, engine opt-out and default-route note; canonical owned regular single-link secret files are unlinked without reading bytes. Native directories/keyring entries are untouched. Unsupported defaults documents retain Python before admission. |
| `apply_default_preset` | Rust for absent/public-payload backing files (also empty-object routing) and a valid connection registry; absent-only seeding including conditional default-profile capture. Other existing documents retain Python diagnostic behavior. Existing profiles, even malformed, are never overwritten. |
| `reset_default_preset` | Rust; literal confirmation, freshly observed pair revision, private backups and in-process rollback. |
| `refresh` | Rust for the prelaunch subset and qualified Codex/Claude native-account or secret-file launches, including qualified two-argument Codex/Claude installs, [described below](#native-launch-capable-catalog-producer). These observe without prompts and publish immutable snapshots. Unqualified Codex installs, OpenCode, keyrings, missing locks and unqualified roots/projections retain the whole Python request before effects. |
| `install_engine` | Rust for every Codex/Claude request and the pre-effect refusals (unsupported engine; OpenCode needs its native installer). It installs only the manifest pin from `pinned-dependencies.json`, [described below](#native-pinned-engine-install). Python is kept before any effect only when an OpenCode binary candidate exists (its version probe in the reply runs in the namespace sandbox), a native login marker is not projectable, or the connection registry does not parse. |
| `onboard_opencode` | Rust for the pre-effect refusals (missing `explicit_opt_in`, invalid `expected_revision` or `name`) and, when no OpenCode binary candidate exists, for the owner-lock and revision check followed by the retained `opencode_not_found` refusal. With a candidate (`~/.opencode/bin/opencode` or `opencode` on PATH) the request keeps Python: discovery runs the binary's version probe in a network-less namespace sandbox, then verification, registration and a catalog refresh follow. |
| `profile` / `read`, `create`, `save`, `update`, `delete`, `deactivate` | Rust; complete snapshot reads, revision-locked capture/metadata/selection, format-1 backup/migration and verified deletion. `update` implements rename and description editing. |
| Wider profile numbers | Rust for ignored top-level document metadata (including nested arrays/objects), with exact integers and Python-normalized nonfinite/overflowing floats. Wider modeled fields, all backing files and numeric tokens over 4,300 bytes retain Python before admission. |
| `profile` / `activate` | Rust for qualified format-1/2 profiles and any subset of routing, defaults/ceiling, MCP usage policy and connection limits, including all four. Pending journal, four revision checks, modification/discard/recovery and installed-definition preservation remain. Unqualified/malformed backing documents retain Python. Routing activation still requires integer model-policy version 2; the current Python implementation rejects missing/other versions too. Those refusal shapes retain Python. |
| `profile` / unknown verb | Rust: `unsupported_action` naming the verb with Python `repr` (`None`, `True`/`False`, integers, ASCII strings); a list/object verb is Python's `machine_tool_failed`/`TypeError` reply. A float or non-ASCII verb keeps Python: the socket's JSON parser loses the float literal, and non-ASCII escaping needs Unicode printability. |
| `connection_update` / other operation | Rust: revision syntax, connection id, then `unsupported_operation`, in Python's order. |
| Unknown/missing action | Rust: `unsupported_action` ("This machine does not support that yet."); a list/object action is `machine_tool_failed`/`TypeError`; truthy non-object params answer the `machine_tool_failed` RPC error. |

#### Native pinned engine install

Settings **Download engine** (`config.action` `install_engine` for `codex-acp` or
`claude-agent-acp`) runs in Rust with the retained
`engine_update.install_pinned_engine` steps: the qualified manifest pin; a current
channel bundle reports that version without npm; otherwise `node` is found on the
service PATH and `npm` beside it (or on PATH). Under the owned root's `.lock`
(nonblocking: a concurrent install answers "unavailable, busy or failed
verification"), stale `.tmp-*` directories are removed, an existing pinned
install is verified without npm, and otherwise npm installs into a private
`.tmp-<version>` staging with the retained environment, npm's directory first on
PATH, cwd `/`, output discarded and an absolute kill deadline within the machine request budget. The staged manifest
version, its contained declared `bin` (and, for an SDK-override Claude build, the
SDK version) are verified before the rename; failed verification removes the
staging, and a failed or timed-out npm leaves it for the next pass. The two newest
installs and every qualified one are kept. Success records the `downloaded`
first-run status (best effort) and returns the native setup discovery, with
OpenCode `not_downloaded` because admission found no candidate. The service's
30-second machine deadline includes queueing and preparation. npm uses the remaining
budget, capped at 29 seconds, reserving one second for kill/reap and the reply;
an exhausted budget launches nothing. A timed-out npm is killed and reaped before
the install refusal returns, with no installed version or downloaded status published.
These steps
duplicate the Rust ACP first-run installer, which cannot be shared because that
crate depends on the service.

The recorded offline capture `crates/workspace-service/tests/fixtures/engine-install-python.json` (32 install and 6
no-candidate onboarding cases, fake `node`/`npm` executables, no network; recorded once from the former Python implementation, whose generator was removed) is
compared with complete replies, npm argv/PATH and home/config trees including
modes. Real npm downloads and a real bundle are not exercised.

#### Native prelaunch catalog refresh

This is a read-only subset of explicit catalog refresh, **not** a native engine
observation producer. It uses the shared existing `connections.lock` without
creating a lock, ignore file or configuration directory. Registry bytes are
checked across lock acquisition and again after composing the complete catalog.
A changed launch shape after admission refuses without switching providers.
The request's `expected_revision` and supplied paths/commands remain ignored,
matching the existing refresh dispatcher; they cannot choose a storage target.

For null/relative commands the outcome is
`engine_unreachable:PortableError`; for an empty command list it is
`engine_unreachable:IndexError`. An existing read-only engine-state directory
instead yields `engine_home_read_only`, matching Python's metadata-only failure
classification. No credential or marker contents or keyring backend are read.
Existing snapshots remain byte-identical and the returned catalog remains cached:
failed attempts never become successful observations.

The recorded capture `crates/workspace-service/tests/fixtures/config-catalog-refresh-python.json`
holds 21 cases recorded without processes, network or keyring access (its Python generator was removed and the capture can no longer be reproduced). Socket tests
compare complete replies and complete configuration/home/catalog trees and modes;
19 admitted cases require zero helper calls, two retained cases exactly one.
The additional launch-capable boundary is described next. Installer and OpenCode
onboarding requests outside the pre-effect refusals, and valid keyring-backed validation, still retain Python.
**Settings still routes unported actions to the lazy Python provider.**

#### OpenCode pre-probe rejection

The native prelaunch subset also rejects OpenCode commands whose nonempty,
absolute, NUL-free argv cannot resolve an install before any version probe:
wrong arity/subcommand, absent binary, noncanonical path, known untrusted
codeg root, nonregular/nonexecutable binary, or binary ownership/write-mode
rejection. Metadata/I/O cases outside this classifier retain Python. The shared
owner lock, registry-byte fences and qualified cached projection still apply.
These are failed attempts (`engine_unreachable:PortableError`, or the existing
read-only-home classification), not successful install/version observations.
They create no engine logs, jail, provider files or snapshots and read no binary
or credential contents. A newly launchable install after admission refuses
without switching providers.

**All potentially usable OpenCode installs still select Python before effects.**
Native OpenCode version probing, successful verified launch, process-isolated
jail, owner-provider/config import and Cortex-mode/per-model observations have
not been implemented or qualified. No fake jail substitutes for that boundary.

#### Native launch-capable catalog producer

Rust observes `codex-acp` and `claude-agent-acp` connections with an absolute
recorded executable and native-account or API-key/secret-file authentication.
Direct commands and two-argument commands without package metadata are admitted.
Two-argument Claude installs additionally require canonical regular node/entry/
manifest files, executable node, a nonempty version and a matching declared bin.
Their bytes are rechecked before spawn; manifests are bounded to 4 MiB and
streamed content hashes to 512 MiB per regular file.
Qualified Codex package candidates now resolve nested or npm-hoisted Codex and
platform packages on the qualified Linux x86_64 path, verify the executable bundled binary,
and create the same private manual-review adapter, dependency symlink and optional
xdg-open bytes/modes as Python. The runtime-launcher content intentionally changes
to a native-interpreter artifact; its private 0700 mode and atomic writes remain. Captured positive adapter
versions are 1.12.0, 1.13.0, 2.0.1 and 2.1.1. Unsupported patch/version/layout
shapes return the captured failed outcome without launching a peer.
The exact-hash 2.3.0 and 2.6.0 bundles are now patched natively as well (2.6.0,
SHA-256 `733b7521…db98b99`, re-derives four of the 14 rules for its permission-profile and
effective-config call sites; see [Codex adapter support](features/codex-engine/index.md#current-support-260-2026-11-16)). Its rules are the
ones recorded from the former Python adapter patch (the generator was removed), and an
offline comparison against the real upstream bundle (SHA-256
`d584cb7d…a3d8870`, installed under the owned engine directory) produced a
patched adapter byte-identical to the recorded former patch. The producer still requires that
digest and exactly one match of every patched call site; any other 2.3.0 bundle
returns the failed outcome without launching a peer. A unit case qualifies the
success and the digest refusal with a synthetic bundle standing in for that digest.

Two-argument Codex package candidates that Python's install resolver certainly
cannot build are admitted as **unverified direct launches**, as Python does:
no resolvable `@openai/codex` manifest, or (Linux x86_64 only) a resolvable
manifest without a vendored `codex-linux-x64` binary. Python then launches the
recorded argv with no install, so Rust also writes no private adapter or
launcher, sets no `CODEX_PATH`/`NO_BROWSER` and runs no `debug models`
enrichment; environment allowlist, bounds, process-group cleanup, logs,
snapshots and replies are the direct-launch ones. The entry must be absolute and
lexically normal, and the adapter and Codex manifests bounded regular
non-symlink files that parse as JSON, so Python's parser cannot fail outside its
caught errors. The shape is re-classified at launch. The background first-run
observation of the default Rust ACP entry uses the same producer, so a pinned
owned Codex install without its bundle is observed instead of reporting
`native_catalog_unavailable`. Each new session of that entry also refreshes
every registered connection once through this producer, prompt-free, like
Python's per-session refresh; shapes it does not admit are skipped, and an
unfinished refresh ends with the process. A buildable install whose outcome depends on
Python's verifier (trust roots, symlinked components, executability, drift,
legacy bin shapes), malformed/symlinked manifests and other platforms keep
Python (`codex_install_outcome_unqualified`).

After a successful model observation, Rust runs only the verified binary's
offline `debug models` command and overlays visible per-model efforts; models
absent from that response keep their ACP-observed efforts. Malformed/empty JSON,
nonzero exit or a reaped timeout skips this optional supplement. Each output
stream is bounded to 4 MiB; overflow/I/O or uncertain cleanup refuses publication
without provider replay. No debug output is retained. The debug command matches
Python's ambient environment/cwd, not the ACP secret-injection environment.

The generated launcher now uses the absolute path of the compiled binary that
wrote it (`workspace-service`, `cortex-acp` or `cortex-paseo-tools`) as its Linux
shebang interpreter (`--native-codex-runtime`), with an exactly validated JSON
binary-path payload. Each of those binaries handles that mode first, through the
same shared implementation, and the artifact is accepted only when its header
names the running interpreter itself. It uses neither Python nor a shell/PATH lookup. Only
`app-server` is accepted (otherwise exit 64). Rust preserves the ordered feature
flags and MCP-disabling overrides, inherited allowlisted environment/cwd/stdio,
Python's C-locale coercion, 20-second checks, generic exit-65 check failure, and
final exec/PID/exit/signal behavior. The existing parent environment allowlist is
unchanged; this mode is not a socket or model operation.

Safe existing Python launcher artifacts migrate atomically during the next
qualified refresh; already-open old files remain intact. Identical native files
are reused. Unsafe leaves are not followed, and an uncertain write never retries
or switches providers. A missing/relocated service executable cannot run the
artifact; a subsequent qualified refresh regenerates its absolute interpreter
path. Unsupported interpreter paths (whitespace/non-ASCII or a shebang longer
than 255 bytes) retain Python before effects. The preexisting bundled-binary
path restrictions still apply. This removes the launcher dependency for the
qualified catalog slice, not the retained Python engine/session or Settings paths.

39 actual native/Python execution comparisons use allowlisted native fake peers
in disposable roots, with no Python on the native launcher's PATH. They cover
argv/cwd/environment, feature/MCP rejection, stdio, timeouts/reaping, final PID,
status and signals. The 67 catalog comparisons validate the exact legacy
launcher template before comparing the new artifact; all other bytes/modes,
replies and transcripts remain compared unchanged.
`crates/workspace-service/tests/fixtures/codex-launcher-golden.json` retains the patch rules and launcher template recorded from the former Python oracle (its generator was removed).
`timeout 90 cargo test --locked --offline -p workspace-service --test codex_launcher`
runs the launcher execution comparisons.
Package candidates outside canonical regular/matching-bin/size bounds, non-ASCII
or control-bearing bundled-binary paths and unsafe existing artifact state retain
the whole Python request. Existing connection-state ancestors may be owner-owned
0755, matching Python's parent creation beneath the private Cortex state root;
private adapter directories and files remain 0700/0600.

Native-account references use existing canonical directories. API-key launches
read only the id-derived private regular single-link secret file and inject the
one engine-specific variable (`CODEX_API_KEY`, `ANTHROPIC_API_KEY` or
`CLAUDE_CODE_OAUTH_TOKEN`), never argv, replies or lifecycle metadata. A missing,
unsafe or invalid-UTF-8 secret produces a failed attempt. A noncanonical
filesystem reference produces a failed attempt without reading its target or
starting a peer; normal ignore/log effects still occur. Registry bytes and old
snapshots are unchanged. Keyrings are not converted into files or another backend.

Unicode connection names use the Python-compatible safety allowlist after NFKC
normalization. Native labels use retained Unicode 15.0.0 tables, control/format
removal, combining-mark handling, whitespace folding and code-point length caps.
Model names/descriptions and original model identities remain unchanged.
Config/state parents must exist as real directories; existing Cortex roots must
be private and outside project Git trees (the home-dotfiles exception remains).
Conflicting ignore files, credential-path collisions, missing private owner locks,
unsafe secret parents or unqualified cached projections retain the whole Python
request before effects.

The producer uses a private disposable empty workspace, a fixed environment
allowlist and only registered argv. It sends `initialize` and `session/new`,
never `session/prompt`, forwards no MCP servers or channel credentials, and reads
no native marker contents. Claude selects each model to observe its efforts and
restores the initial selection. Observation has a 20-second overall budget;
cleanup has bounded waits. Stored authorization state is never promoted merely
because an engine answered.

A validated observation publishes a fresh private `cs_` snapshot. Linux
publication fsyncs staging bytes, uses no-replace atomic rename and syncs the
directory; existing snapshots are never replaced. The shared owner lock spans
observation/publication. Exact registry bytes are fenced across lock acquisition,
before publication and after projection. Uncertain effects never switch providers
or automatically replay; earlier per-connection publications may remain.

Noisy peers now retain scrubbed diagnostics: credential-shaped text is redacted
before writing, each line retains at most a 400-character prefix plus truncation
metadata, and the private log is capped at 256 KiB, matching the qualified Python captures. UTF-8 decoding is lossy. A line
over 64 KiB, failed diagnostic I/O or a known launch secret echo refuses
without that connection's snapshot or provider replay; no raw diagnostic is
persisted by those refusal paths.

Qualification uses the recorded capture
`crates/workspace-service/tests/fixtures/config-catalog-producer-python.json` (its Python generator was removed):
67 captures of the former Python dispatcher/producer (54 prior cases, eight OpenCode
pre-probe failures and five unverified Codex direct launches: no dependency,
hoisted or nested dependency without a binary, API key, authorization required), with allowlisted synthetic shell ACP peers only, no
network/accounts/keyring. Socket tests compare complete replies,
surviving home/config/catalog/log bytes and modes and exact peer transcripts with
zero helper calls. Disposable roots, fresh snapshot/engine identities and clocks,
scratch paths, argv digests and independent EOF/reap ordering/poll races are
explicitly bound before normalization. Failed outcomes and terminal exit codes
are not normalized into success. Unicode tables and the former Python normalization/label
vectors are recorded in `crates/workspace-service/tests/fixtures/catalog-unicode-python.json` (its generator was removed).

**Remaining boundary:** unqualified Codex/Claude package candidates (including verifier-dependent
Codex installs, unresolvable Claude packages and other Codex platforms),
all potentially usable OpenCode launch/verified-install/jail/version/provider/config/mode
paths (apart from the metadata-only pre-probe rejections above), OS-keyring launches,
other auth/reference combinations, unsafe names/roots, missing locks and
unqualified projections keep Python before effects. Claude same-name models
without observed efforts still refuse after observation without replay.
Finite captures do not prove universal malformed/noisy transport parity.
Installer/onboarding requests outside the pre-effect refusals, valid OS-keyring validation, custom-document preset apply,
unsupported removal-default documents, wider modeled/backing-file profile
numbers or over-bound tokens, unqualified activation documents and unknown
actions/verbs still use the lazy Python Settings provider.
This is not the completed Settings migration.

Native metadata edits retain canonical registry bytes, omitted-field normalization,
sorted opt-outs and arbitrary numeric history, including large integers, floating
point, negative zero and legacy nonfinite history. The configuration-root `.gitignore`
is created privately with exactly `*` and a newline, or checked without replacement.
A conflicting/unsafe ignore file refuses before the registry lock or write. Valid
admission can create this ignore file and the owner lock even if a later revision
or registry check refuses, matching transaction setup effects.

Trusted service configuration selects roots; socket root/refresh parameters do
not select a configuration directory. Native metadata uses the shared owner lock,
exact revisions and complete redacted replies. Private bounded-file safety and
reply budgets fail closed; registry replacement uses fsync, atomic rename and
parent-directory sync. Post-replacement uncertainty never delegates or replays.
Explicit producers may contact engines, keyrings or the network as before; opening
settings does not invoke them. No second project owner or scheduler is started.
Cooperative locking cannot isolate hostile same-user writers. Errors, timeouts,
disconnects and unknown outcomes never trigger automatic provider switching or
replay; inspect current settings before retrying.

Offline captures retain 23 original and 46 additional real Python metadata cases.
Unit comparisons include the fixed-clock no-op, complete replies, registry bytes,
all configuration-directory files and modes. Socket tests compare 22 original and
all 46 additional cases and assert that selected metadata variants never invoke
Python, including refusals. Three remaining variants retain named offline
producer fakes; these prove routing, not installation, credential validation or
engine discovery. All use disposable HOME/XDG and process/network/keyring tripwires.
Further socket checks retain real preset application, native profile reading, reset
confirmation refusal and one unknown retained effect without replay; native unit
checks exercise concurrent stale revisions, unsafe targets and post-commit uncertainty.
Fourteen additional real Python reset captures compare complete replies, every
configuration-directory file byte and mode, including malformed existing files,
unchanged resets (which still back up), literal confirmation and ignored caller
revision/root fields. Native socket reset cases require zero helper calls. Existing
oracle captures are unchanged; only generated backup timestamps and trusted host
paths are rebound. Unit checks also cover reset rollback/removal, failed rollback,
shared-lock revision conflicts, unsafe targets and uncertain completed writes.

Native reset uses the build-embedded `policy/model-settings-default.json`, with
insertion-order serialization matching Python. The service re-reads the pair before
confirmation and checks that fresh observation under the shared lock: as in the
retained machine wrapper, a caller-supplied `expected_revision` is ignored. MCP,
profiles and connections are not rewritten. Native private-file/reply limits still
fail closed. Each replacement and backup is atomic and directory-synced, not a
single two-file transaction; an unknown durability outcome is not replayed or
silently rolled back. Existing backup-name collisions refuse instead of replacing
an earlier recovery copy. No engine, keyring, network or project store is contacted.

Connection validation/removal adds 71 real dispatcher captures with complete replies,
configuration/home/state file contents, symlink targets and modes, and zero helper
calls per native socket case. Filesystem validation never opens credential contents;
unchanged state preserves exact original registry bytes. Non-secret removal commits
record/history deletion and sorted opt-outs in one atomic registry replacement.
The selected default-route note is prepared before commit and its observed defaults
revision is rechecked. Unlike the retained two-lock wrapper, native admission holds
the same owner lock through revision checking, derivation and replacement; this
closes its intervening writer race without weakening the revision fence. Only
representable defaults documents are admitted for native removal; malformed or
unsupported defaults keep their Python behavior. Private-file/reply limits still
fail closed. Pre-commit failure preserves registry bytes; post-replacement uncertainty
never rolls back by guess, delegates or retries. Valid keyring validation remains a separate Python workflow with unchanged backend policy (stale-revision refusals are native);
API-key creation uses the native workflow described above. Secret removal now uses descriptor-relative metadata and
unlink, never opening credential contents. A linked/public parent refuses; leaf
links, directories, multi-link files and noncanonical references are not deleted.
Deletion still precedes registry replacement, as in Python: there is no secret
rollback. A later registry failure is explicitly unknown, never a claim that the
credential was restored or an invitation to replay.

Native profile mutations select only the trusted owner root and check the exact
profile revision under the shared owner lock. Create/save also check all four
backing-file revisions and capture normalized snapshots, excluding MCP command,
args and env. Read stays noncreating; update/delete/deactivate never rewrite
backing settings. Format-2 metadata mutations replace only routing-profiles.json, with no
extra backup (matching Python). First changes to format 1 retain a private
.migration-backups copy, verify migrated bytes and untouched profiles, and restore
on failed verification. Delete also verifies/restores its single-file change.
Failed restoration is explicit; uncertain durability never rolls back by guess,
falls back or replays. Backup-name collisions refuse. Private-file/reply bounds
fail closed. Wider numeric metadata in ignored top-level profile-document keys is
preserved natively, including nested values, without rounding. It is not a modeled
setting and never enters a reply as a numeric surrogate. Wider numbers in modeled
fields/backing files or tokens over 4,300 bytes still select Python passively. If a
file changes into an unqualified numeric shape after admission, the locked parser
refuses rather than rounding or replaying.

The two recorded captures `config-profiles-python.json` and `config-profiles-edges-python.json` (under `crates/workspace-service/tests/fixtures/`; their Python generators were removed) retain 125
cases, including success, refusal, interrupted selection, format-1 migration,
malformed snapshots and ignored caller roots. Unit tests compare fixed-clock
replies and all file bytes/modes. The config_action_service socket test compares
complete replies, every file name/content/mode and backup-directory mode, with
zero helper calls per case. Only validated native timestamps/backup names and
trusted host paths are rebound; revision hashes are recalculated from the exact
expanded expected bytes. Native fault tests cover changed revisions under the
owner lock, unsafe files/locks, backup collisions, verification rollback, failed
restoration and uncertain completed writes. Qualified format-1/2 activation prepares
all candidates before journaling pending, then writes changed sections in fixed order.
Defaults retain unknown metadata and merge the owner ceiling; MCP definitions/env
stay installed while usage policy changes and extra servers are disabled; registered
connection limits change without validating accounts, and missing connections produce
notes. Format-1 journal publication is backed up and verified before backing writes.
A known backing failure restores earlier completed writes in reverse order and clears
pending best-effort. Unknown writes, failed rollback or finalization stay interrupted/
unknown without replay. Missing/other routing model-policy versions are rejected by
current Python, not silently migrated; their existing provider is retained.

Three additional immutable recorded captures cover 19 preset-apply, 24 routing-only
activation and 12 secret-removal cases:
`config-apply-python.json`, `config-activate-python.json` and
`config-secret-remove-python.json` under `crates/workspace-service/tests/fixtures/` (recorded from the former Python implementation; the generators were removed).
Socket cases compare complete replies and file trees/modes and require zero helper
calls, including refusals. Existing captures are not rewritten. Native fault checks
exercise journal/target/finalization failures, no-overwrite publication races,
secret hardlinks/linked parents and post-unlink registry failure.

Additional immutable recorded captures are `config-activate-full-python.json` (including invalid-language refusals),
`config-activate-materialize-python.json` (valid four-section materialization)
and `config-profiles-wide-python.json` (wider ignored metadata and bound-route
activation), recorded from the former Python implementation (the generators were removed). The socket compares full replies,
file bytes/modes and backup directories, with zero helper calls for every admitted
case; separately retained policy/malformed-document refusals call Python once.

Preset apply uses complete private temporary inodes and no-replace hard-link
publication under the owner lock. It never repairs/replaces existing files. Its
configuration files are individually published, not one transaction: a failure or
hard kill can leave an earlier subset created. No rollback is claimed; a concurrent
publication or uncertain directory sync requires inspection. No engine, network,
keyring, credential read or project-store change occurs in these native producers.

There are no separate MCP add/remove/validate/enable verbs in config.action.
Those Settings edits already run through native config.save/mcp_servers (whole
registry edits with validation); explicit config.mcp.discover is also native.
Neither a new MCP action spelling nor a profile validate verb is invented here.

### Native machine saves

Machine `config.save` selects Rust for all three surfaces before any effects;
it is removed from Python migration classification and helper admission. There
is no fallback on malformed input, refusal, disconnect or uncertain persistence.
Trusted service HOME/XDG/channel roots supply paths, never socket fields.
Saves do not open a project store, claim scheduler ownership, refresh credentials
or launch engines/MCP definitions.

The socket regression in
[`config_save_defaults_service.rs`](../crates/workspace-service/tests/config_save_defaults_service.rs)
compares 224 complete reply/file observations from disposable offline Python
captures: 56 defaults and 168 routing/MCP observations. A forbidden helper
asserts zero Python invocations. The five defaults AttributeError cases also
have real retained-provider socket observations: clients received the ordinary
JSON-RPC `-32000 / machine_tool_failed` error, not an exception name. Rust
preserves that envelope without crashing. Other ordinary handler errors retain
their existing `ok: false` machine result. Unknown surfaces remain unsupported.

Native storage/frame/nesting/exact-number bounds remain deliberate fail-closed
admission differences, not unrestricted arbitrary-JSON parity. Unsafe/oversized
state, unrepresentable mapping keys and preserved out-of-range integers refuse
rather than lose data. The parser's registry diagnostics remain native.
This qualifies source/socket saves, not an installed app or the all-Rust product.

### Native machine-routing persistence

The internal `PreparedUserRouting` transaction in
`crates/workspace-service/src/machine/project_config_save/user_routing.rs`
prepares user routing bytes under the shared private owner lock. It preserves
the editor conversion, exact/fallback revisions, validation and unknown metadata,
then commits by private fsynced atomic replacement. Dropping a preparation does
not save settings. A failed directory sync after replacement stays unknown.

The native `user_routing::save` section now composes the complete machine-global
routing reply, cached model efforts and post-save profile selection against the
candidate bytes before commit. Offline Python comparisons cover complete replies,
exact bytes/modes, environment shadowing, observed/conflicting/default-only efforts,
format-1/2 profiles, modified/interrupted/unavailable selection and damaged/newer
profile files. A profile failure remains best-effort status, not a save refusal.
The configuration-root ignore-file effect uses the defaults section's helper.

This section is selected by machine `config.save` for routing. Trusted host
configuration selects roots; JSON cannot refresh an engine or select another
directory. No Python fallback or route authorization is introduced.

### Native machine-defaults section

The internal `user_defaults::save` section now converts and validates the machine
editor's portable coordinator, bound route, language and ceiling, preserves
uneditable defaults/metadata, and builds the complete management update reply.
It also prepares Workspace mode/checkout while retaining unrelated Workspace keys.
Offline comparisons cover complete replies, both files' exact bytes/modes,
no-ops, revisions and the private configuration-root `.gitignore` effect.

The shared owner lock and Workspace `machine.lock` cover candidate preparation
through atomic fsynced replacement. Both candidates and the complete reply are
budgeted, and observed revisions are rechecked before writing. Invalid or unsafe
Workspace storage refuses before either settings file is changed; this is
deliberately stronger than the former provider's possible partial write.
An error after the owner file commits is unknown, never a reason to replay or
switch provider. The two files are not one atomic transaction. Directory/lock
creation and ignore-file creation can precede a settings refusal.

This section is selected by machine `config.save` for user defaults, including
Workspace mode/checkout. All 56 complete captured observations now pass through
the native socket regression, including malformed inputs; no Python helper,
engine, project store or keyring is used.

### Native machine-MCP section

The internal `user_mcp::save` section implements the editor's implicit removals,
ordered whole-entry upserts, duplicate-name updates, env keep/change/remove,
revision fallback/conflicts and complete redacted replies. It preserves the
`servers`/`mcpServers` root, unknown root metadata, entry order and private modes.
An empty registry with no requested servers returns only `changed: false`, without
a token check or creating settings/ignore files, just like the former provider.

The section reuses the shared owner lock, defaults ignore-file helper and atomic
fsynced replacement. Candidate bytes, loader validation and the whole reply are
checked before saving; observed drift refuses. A post-rename sync failure stays
unknown, never triggers a second write or provider switch. Offline captures use
disposable HOME/XDG with process/network/keyring tripwires; definitions are never
launched and env values never appear in replies. Native tests also cover unsafe
files, reply limits, concurrent original revisions and uncertain durability.

Routing, defaults and MCP are selected together by the native machine writer.
Unknown-effect and single-owner fences are unchanged. The `config.action` shapes
listed above as retained (keyring lookups with a startable provider, unqualified
catalog/profile/preset documents, OpenCode candidates, float or non-ASCII profile
verbs), plus legacy projection/evidence paths, are refused with `config_api_unavailable`
(the on-demand Python provider was removed). See [startup](cortex-workspace/local-task-execution.md#python-independent-native-service-startup).
No installed application or engine registration is changed.

### Manage settings in Paseo

For the default Rust launch, the sibling `cortex-paseo-tools config` answers the
complete API described below natively, with no Python fallback: every read, and user
defaults, routing, profiles, MCP servers, connections, the project ceiling, the
default preset, model-list refresh, engine discovery, onboarding and download. It
uses the owner's configuration lock and revision checks, writes the same files as
the Python launch, and does not write Workspace-specific defaults. Plugin activation
runs its native automatic setup. A few shapes only the Python provider can answer
(an OS-keyring connection check, an installed OpenCode binary's version probe, a
model-list refresh for an engine with no qualified native catalogue producer) refuse
with `config_api_unavailable` and say why; nothing is changed. See
[Settings under the Rust launch](project/native-incoming-acp.md#paseo-settings-under-the-rust-launch).

The plugin implements a **Cortex Settings** screen for supported Cortex configuration; this is not Paseo's plugin-settings document. However, independent desktop checks on installed Paseo 0.10.0 found the plugin enabled and running but did not find its screen in Settings or its command in the command palette. Earlier implementation/test evidence describes the screen, while live access in the tested host is inconsistent and remains unverified; do not assume an owner can reliably reach it. The screen has eight sections: user defaults, Models, Engines & models, project ceiling, routing, user routing profiles, MCP servers, and connections. **Models** switches individual models on or off for Cortex as a whole (routing `enabled` and exact `deny` entries, below) and has a **Refresh model lists** button that re-reads every connection's model list from its engine; **Engines & models** lists only the models switched on there. Editable fields use typed controls (such as text, number, selectors, switches, and lists), not JSON text editors; values the form cannot safely represent are read-only rather than rewritten. Select a project for project-scoped settings: the picker lists one entry per project directory (project name and path, worktrees marked), built from Paseo workspaces, whose titles are task titles and are not shown. Every field or group of similar fields (rows, per-model controls) has help that says what it does and recommends a setting; the **Help language / Язык подсказок** selector at the top shows it in English, Russian or both (default both, remembered by the client when local storage is available). The help texts live in `integrations/paseo/client/settings-help.ts` and name no model. Ceiling values this version accepts but does not apply — `permission_level`, the `calls`/`tokens`/`concurrency` limits and the spend limit — are not shown as fields; stored values are kept unchanged on save and listed in a read-only note. Only the `attempts` limit is editable. This is a bounded management interface, not a control for every runtime or task setting. Use the documented setup CLI as a fallback where a supported command exists; for OpenCode onboarding, see [the OpenCode adapter](features/opencode-engine/index.md#onboarding). See [verification guidance](project/verification.md) for the desktop findings and untested flows.

In **Engines & models**, the engine switch adds/removes a `<engine>/*` token in `deny`: it disables model selection and dispatch for that engine but does not remove its connection or credentials. This is a routing rule, not a connection-registration toggle: use the Connections section or setup command to register a connection; removing one is a separate operation. The separate automatic-routing switch edits `providers`, which restricts automatic routing, failover, and pins but does not hide models from manual selection. Per-model switches edit `deny` (including selector visibility and dispatch) or `auto_allow` (lifting only a built-in automatic exclusion for that exact model). `deny` takes precedence; provider policy, catalog and connection readiness, and other runtime eligibility checks still apply. The screen shows effective status and matching rules. Project routing takes precedence over `$CORTEX_ROUTING`, then user routing; the first existing file wins and files do not merge. Review shadowing warnings before saving. A newly created project file is seeded from effective rules, not created empty.

The `routing` surface carries the `models` key (per-engine automatic-routing selection, tiers and per-model policy). In **Engines & models**, each discovered model has an automatic-ranking switch, a multi-select of light, standard, deep and flagship tiers, and work-kind preference switches using the runtime's work-kind labels. A preference can be selected only for a currently auto-eligible model or one explicitly selected in `models.<engine>.auto`; excluded models require that explicit selection first. Saving writes the selected engine/model values and `preferred_for` lists through the revision-checked routing API, preserving other per-model policy attributes. The read-only `route_catalog` reports each route's `tiers` (every tier it is a candidate of, strongest first), `tier_source`, engine `selection` mode, `auto_selected` membership, `configured_tier`, effective `policy`, and `not_selected` reason, plus every engine's `selected_models` and `unmatched_selected` ids. Each route also reports `enabled` (routing `enabled` or its per-engine default) and each engine its `enabled_models`, `enabled_default`, `unmatched_enabled` and, per connection, `catalog_observed_at` (when the engine last reported its model list; `null` when never).

In **Routing**, pin effort, `effort_floor`, `max_effort` and the ordered `effort_ladder` offer the exact engine/model's cached observed effort levels. An observed empty selector offers only engine default or neutral/unset. Missing observations (including hand-typed ids), damaged snapshots and conflicting connection observations use a visibly marked global vocabulary fallback, not a claim of model support. Refresh model lists in **Models**, then reload Routing to obtain updated choices. Existing unsupported values stay visible and unchanged until explicitly removed or replaced; ladders permit adding observed levels and reordering/removing stored entries. The routing view's `builtin.model_efforts` maps engine to exact model to levels (`[]` for default-only; missing or `null` for unknown). Reading it never refreshes engines or changes settings.

User profiles snapshot current owner settings, not just routing. Edit working settings through their own sections, then explicitly save into the selected profile or create a new profile. Ordinary settings saves do not overwrite the saved profile. Activation restores its captured user sections; project/environment routing still takes precedence. See [User routing profiles](#user-routing-profiles) for states, revision checks and restore behavior. Rendered access remains subject to the host limitation above.

The owner API is version 1 and is served natively. The typed plugin RPCs are `cortex.config.read`, `cortex.config.update`, `cortex.config.opencode.discover`, `cortex.config.opencode.onboard`, `cortex.config.connections.discover`, `cortex.config.connections.onboard`, `cortex.config.connections.install`, `cortex.config.preset.read`, `cortex.config.preset.apply`, `cortex.config.preset.reset`, and `cortex.config.catalog.refresh` (the management action `refresh` on the read-only `route_catalog` surface: a prompt-free re-read of every connection's model list, returning the fresh catalog with one outcome per connection). The plugin runs this API through its selected Cortex launch, using that launch's HOME/XDG configuration. Projects are resolved from the selected Paseo workspace; requests do not supply paths, commands, or binaries. Paseo RPC invocation is gated by its daemon authorization, and writes require the read's current revision. A stale revision returns a conflict; reload the latest value before retrying rather than overwriting it. A timed-out write can have an unknown outcome: re-read before retry.

The API is intentionally closed rather than a general JSON editor. It patches supported user defaults and ceiling fields; replaces supported routing keys while preserving unknown keys; uses the existing project SQLite row for the project ceiling; and validates MCP and connection updates with runtime loaders. It never writes the project task/report database except that existing SQLite configuration row. Routing precedence remains project `.cortex/routing.json`, then read-only `$CORTEX_ROUTING`, then user `routing.json`; sources do not merge, and a UI write to a shadowed file is surfaced as such. MCP environment values and connection credential values/paths are redacted. When editing MCP env, use `{"keep": true}` to preserve an existing secret; the API does not return it.

OpenCode v2 discovery is read-only. Onboarding requires an explicit opt-in and registers the existing owner login only; it does not copy credentials or enable automatic routing. Routing remains excluded unless the owner separately configures an exact OpenCode route under the supported routing rules. See [OpenCode adapter](features/opencode-engine/index.md#onboarding).

The **Engines: download and connect** group under Connections shows installed/login/connection state and automatic setup progress, polling every three seconds while mounted. Plugin or agent startup runs one background pass: download Cortex-owned pinned Codex and Claude adapters, register missing native-account connections when a login marker exists, then observe models immediately without a model prompt. Existing connections are not replaced. Removing a connection atomically records its engine in `connections.json`'s optional `auto_connect_disabled` list, preventing automatic recreation, including a concurrent startup. **Add connection** can restore it explicitly with the opt-in switch, current revision, qualified installed adapter and native login marker. **Download engine** remains a manual retry. These actions accept no key, path, command or version. `opencode` stays manual through **Enable OpenCode v2**; Cortex never downloads or automatically registers it. A login marker is only an existence check, not verified authentication; **Validate** checks connection state and model discovery can still fail. A failed model discovery retains the connection and previous catalogue; use **Refresh model lists** to retry. API-key connections remain setup-command only.

Connections polls retain mounted rows, pending input and removal confirmation; surviving rows keep their order, and new connections are appended. Structurally unchanged reads do not update client state. Failed background reads retain last known connections/engine status with a visible warning and retry controls; they do not clear save conflicts or retry writes. Explicit conflict reload still discards edits.

For API details and current restrictions, see the native settings API (`crates/workspace-service/src/machine`, served by `cortex-paseo-tools config`) and `integrations/paseo/shared/config.ts`. Changes are consumed by the next Cortex session; they do not retroactively reconfigure active tasks. Saving settings does not restart active Cortex sessions. Conversely, a plugin code update/reload is not needed for a settings edit, but it restarts the plugin process and can close open provider connections; apply plugin updates between active tasks until recovery has been qualified on the deployed Paseo version.

`HOME`, `XDG_CONFIG_HOME` and `XDG_STATE_HOME` must resolve to absolute paths. The user roots cannot be placed inside a project workspace. Cortex creates managed directories owner-only and validates existing files before trusting them. The roots are resolved by the shared host-channel resolver in `crates/cortex-paseo-tools` and `crates/workspace-service`; the launch record is written by `cortex-paseo-tools configure-paseo-plugin-launch`.

## Connections

Use the setup command to add or change connections; do not hand-edit `connections.json`. Settings can also rename, validate and remove connections, and can add a native-account connection for `codex-acp` or `claude-agent-acp` (see [Engine install records](#engine-install-records)). Setup validates the engine install record, creates IDs and canonical credential references, and writes the registry atomically. Credential values are read from a file or stdin, never from command-line text.

The supported engines are `codex-acp`, `claude-agent-acp` and the optional `opencode` (OpenCode v2, see [OpenCode adapter](features/opencode-engine/index.md)).

### Approved adapter following and rollback

At incoming ACP startup, before constructing the production runtime, Cortex
advances existing Codex/Claude connections to the newest **approved installed**
adapter when its declared version is strictly newer than the recorded package.
Approved means source-qualified by `discovery.is_qualified` /
`QUALIFIED_ADAPTER_VERSIONS` and exact instruction-delivery/private-adapter
registrations, actually installed in Cortex's owned engine root with a valid
declared bin. A newer unregistered npm release never wins. Existing byte/digest
and private-patch checks still run before launch; selection is not live qualification.

The same selection runs after a qualified `engine_update.update_engine`
installation. It changes only the launch command and its derived trust
fingerprint, preserving connection IDs, account/credential references, endpoint,
provider, timestamps, limits and all route/model settings. The former startup
credential-copy migration is no longer automatic. Running processes are not
reconfigured; later launches use the new command. An unreadable old package or
nonstandard command is left unchanged rather than guessing its version.

Writes use the existing registry lock, current content-revision check and private
atomic replacement. Exact previous bytes are retained as owner-only `0600`
`connections.adapter-backup.json` beside `connections.json` (latest automatic
change only). Stale revisions abort without a partial change. A sanitized stderr
diagnostic names only engine, old version and new version; startup failures defer
selection and retain existing configuration.

There is no per-connection explicit version-pin field. To opt out for rollback,
set `CORTEX_ENGINE_FOLLOW=off` in the Cortex child environment **before restarting**.
This disables local selection, independently of `CORTEX_ENGINE_UPDATE=off`,
which disables the background download/connection/model-discovery pass. With Cortex stopped and following off,
restore the private backup to `connections.json`, preserving owner-only mode,
or use the existing operator install-record/setup workflow for a retained older
qualified install. Do not restore a backup over newer unrelated settings without
reconciling them. Automatic selection never downgrades. OpenCode records a stable
standalone binary command, not a versioned owned npm path, so no analogous
connection rewrite is needed; its existing version checks still apply.

### Claude ACP adapter updates and instruction-delivery qualification

Claude engine auto-updates may install an adapter release before Cortex has an exact instruction-delivery registration for it. Discovery therefore selects the newest installed version with an exact registration ahead of newer unregistered versions, and pruning retains registered installs as fallbacks. If no registered install is available, diagnostics can still identify the installed version, but critical-role dispatch refuses it before launch. This qualification applies to instruction delivery, not proof of effective model/tool enforcement. Exact current registrations include Codex ACP 2.6.0 and Claude ACP 0.107.0, retaining 2.3.0, 2.1.1, 0.97.0 and 0.88.0 for rollback. Startup follows newer approved installed adapters for existing connections; see the following policy. See [Claude adapter support](features/claude-engine/index.md#current-support-01070-2026-11-16) and [Codex adapter support](features/codex-engine/index.md#current-support-260-2026-11-16) for the current registrations and limits. One owner-requested Desktop check on 0.88.0 (2026-10-08) completed a resumed scenario; its bounded, not-clean evidence is on the Desktop evidence page.
 Authentication modes are `native_account` and `api_key` (the Rust setup registers only the former); `gateway` is represented in the type enum but has no implemented launch path and is not usable. Codex API-key kind is `openai_api_key`; Claude accepts `anthropic_api_key` or `claude_oauth_token`. Native account connections may reference the user's existing `~/.codex` or `~/.claude` directory, or use Cortex-managed isolated engine state. OpenCode supports only `native_account`, referencing the login `opencode auth login` wrote to `~/.local/share/opencode`, and is added only with `add-opencode-connection` (below). The supported combinations are fixed in the connection model and enforced by onboarding.

Typical setup:

```bash
scripts/cortex-paseo-tools.sh status
scripts/cortex-paseo-tools.sh add-connection --engine codex-acp
```

The Rust setup registers native-account connections only: `add-connection --engine ENGINE` refuses `--api-key-file`, `--api-key-stdin` and `--install-record` (API-key connections and install records are not offered). For native login, check the engine's native login state before depending on it. The CLI's `status` output reports IDs and credential kind, not credential values.

The registry JSON is intentionally an implementation format, not a hand-authored config API. Its currently accepted root keys are `schema_version` (integer, exactly `1`), `connections` (object keyed by connection ID), and optional `override_history` (object). Each generated connection record has the fields below; records are written by setup and should be treated as read-only:

- `id`, `name`, `engine`, `provider`: strings; `id` is generated and matches its object key.
- `auth_mode`: `native_account`, `api_key`, or the unsupported `gateway` enum value.
- `key_kind`: null for native account; one supported key kind for API-key mode.
- `endpoint`, `launch_command`: nullable endpoint string and nullable string array; setup derives/approves launch details.
- `credential_ref`: generated tagged object (`engine_native_dir`, `secret_file`, or `os_keyring`); it identifies storage and never embeds the secret.
- `approved_fingerprint`: generated string binding the approved install/connection configuration.
- `state`: `configured`, `authorization_required`, `unavailable`, or `invalid`.
- `qualified`: boolean registration capability marker; it is not proof of live client qualification.
- `created_at`, `updated_at`: ISO timestamp strings.
- `max_concurrent_sessions`: integer from 1 to 64; defaults to 4 when absent in an older record.

The closed fields and defaults are enforced by `connections.registry._parse_registry_bytes/_record_from_dict` and `ConnectionRecord`. Unknown fields fail closed.

### Workspace Dev extracted package

The Linux Dev service and desktop support a native, opt-in launch from a trusted
`cortex-workspace-dev` Debian archive extracted without installation. Use a new,
absolute, user-owned directory `$prefix`, without symlink components. Extract
with `dpkg-deb -x "$archive" "$prefix"`, ensure the resulting prefix is mode
0700, and create its `config`, `state`, `run`, `cache` and `data` directories
with mode 0700 before launching either process. Use an appropriately short
prefix for the filesystem Unix socket.

Set these variables in **both** native processes, not globally in the user
manager or login environment:

| Variable | Value |
| --- | --- |
| `CORTEX_WORKSPACE_DEV_EXTRACTED_PREFIX` | `$prefix` |
| `XDG_CONFIG_HOME` | `$prefix/config` |
| `XDG_STATE_HOME` | `$prefix/state` |
| `XDG_RUNTIME_DIR` | `$prefix/run` |
| `XDG_CACHE_HOME` | `$prefix/cache` |
| `XDG_DATA_HOME` | `$prefix/data` |

From a caller-managed service (for example a uniquely named transient user unit),
run `"$prefix/usr/bin/cortex-workspace-service-dev" "$allowed_root" "$prefix/usr/lib/cortex-workspace-dev/runtime"`.
Use a disposable project under `$allowed_root`. Once
`$prefix/run/cortex-workspace-dev/service.sock` is ready, launch
`"$prefix/usr/bin/cortex-workspace-dev"` with the same values. The desktop does
not start or reload the ordinary systemd unit if the socket is missing or lost
(`extracted_dev_service_unavailable`). Native desktop notifications are disabled
for this run. Missing, mismatched or unsafe roots refuse the opted-in launch; the
release channel ignores the opt-in.

In this mode the service ignores inherited `CORTEX_ROUTING` in both dispatch
and machine settings. The qualification launcher also unsets it in every unit.
The
project registry, routes, settings and socket are separate from the persistent
Dev installation. The package's adjacent `engines` and `voice` trees still
supply its bundled assets. Verify archive identity and contents separately:
startup validates layout, not provenance. HOME and the project-local
`.cortex-dev` naming rule are unchanged; use a separate disposable project and
do not copy native account files. This is not a sandbox for untrusted packages.

### Engine install records

Native Workspace prefers the verified adapter in its channel's Debian bundle.
For non-bundled adapters, release Settings discovery, download and native-account
recheck use the same `$XDG_DATA_HOME/cortex/engines` tree, with the HOME fallback
only when `XDG_DATA_HOME` is unset. Workspace Dev instead uses
`<channel-state>/data/cortex/engines`. These are adapter paths, not native account
paths; switching the Cortex data root does not move or copy a login.

Setup commands install no engine. The Settings **Download engine** action, and the background updater, install the manifest-pinned `codex-acp` or `claude-agent-acp` npm package into Cortex's own owned root, `$XDG_DATA_HOME/cortex/engines` (or `~/.local/share/cortex/engines`). The Settings action is bounded to 60 seconds in the former Python implementation; native Workspace npm instead uses the remaining 30-second machine request budget with one second reserved for cleanup and reply. The action takes no package, version, path or binary from the request, never changes an existing connection and needs no install record. At plugin or agent start, Cortex also registers a native-account connection for an engine that has none once its owned qualified adapter and native login marker are present, unless the owner previously removed a connection for that engine. It observes models in the same background pass; no second session or manual refresh is needed on success. If npm or Node is missing or the install fails, Settings reports that the pinned installation failed and shows no npm output. OpenCode is a standalone binary, not an npm package, has no Cortex installer, and takes no install record: `scripts/cortex-paseo-tools.sh add-opencode-connection [--binary PATH] [--name NAME]` finds a trusted binary, reads its version in a network-less sandbox and registers the connection. `$CORTEX_OPENCODE_BINARY` names a binary for that search. For the npm engines: `add-connection --install-record FILE` takes an operator-recorded install of an engine you already installed by other means (for example `npm install @agentclientprotocol/codex-acp`). It holds absolute paths without symlink components and the SHA-256 of each pinned file. `verify_install()` re-hashes every file before launch. The JSON shape follows `CodexAcpInstall` or `ClaudeAcpInstall`: `node_binary`, `adapter_entry`, `package_json`, `package_version` (must equal the package's own `"version"`) and their digests, plus the bundled Codex binary and version for Codex only. `tests/fixtures/engine/codex/install-record.template.json` shows the shape without real hashes. The field-by-field recipes are in the [Codex adapter](features/codex-engine/index.md#current-runtime-gateway-policy) and [Claude adapter](features/claude-engine/index.md#pinned-install-requirements-manual-recipe-placeholder) pages.

Accepted adapter versions: Codex exactly `1.12.0`, `1.13.0`, `2.0.1`, `2.1.1`, `2.3.0` and `2.6.0`; Claude requires exact instruction-delivery registration (`0.78.0`, `0.87.0`, `0.88.0`, `0.97.0`, `0.107.0`). These are not minimum-version floors. OpenCode accepts any `2.x.x` release, checked from the sandboxed `--version` output before launch and again from the engine's own `initialize` `agentInfo.version`; 1.x, 3.x and malformed versions are refused.

## Default preset

`policy/model-settings-default.json` is the bundled preset. It supplies the starting `routing.json` (engines, enabled models, tiers, per-model policy and quota windows), `defaults.json` (coordinator default and language), and `mcp-servers.json` (disabled examples). A fresh install creates exactly one named profile, `default`, in `routing-profiles.json`, with `active: "default"` and an applied snapshot of all four user sections. Settings selects **Current profile: default** and shows **Applied** (`in_sync`). Cortex writes each file only when it is absent:

- `init-storage` seeds the preset files. `install-paseo-plugin` runs it, and so does the Git-install bootstrap (`bootstrap-paseo-git-install`).
- Every Cortex agent process start repeats the absent-only seeding, so a file that goes missing is restored by the next session. A seeding failure never blocks startup.
- The **Default preset** card in Settings' User defaults section shows whether each file exists. **Apply default preset** appears only while a file is missing and never replaces one.

An existing file is left unchanged even when it is malformed or from an older schema; Cortex does not repair it, merge MCP entries into it, or rename/overwrite saved profiles. If the profile registry is absent, `default` is captured only when routing, defaults and MCP usage policy still match the bundled preset; customized settings are never relabeled as an applied default. This also lets an interrupted first install finish on retry. Preset seeding never adds an engine connection.

For technical writers, the preset prefers Haiku/Luna at low complexity,
Sonnet/Sol/Terra at normal complexity and Sonnet/Sol at high complexity.
These are preferences, not pins; see the [routing details](#fresh-install-owner-default-snapshot).

The MCP examples are `codebase-memory-mcp`, `playwright` (Claude workers only), and `vivaldi` (workers). All start with `enabled: false`; the runtime loader excludes them before launch. Settings lists them as disabled, editable and toggleable. Commands, arguments and filesystem grants contain only `/path/to/...` examples; environment maps are empty. Registering these definitions does not install binaries or browser extensions. Install the corresponding tools, replace example paths and review grants before enabling. The named profile captures only MCP usage policy, never commands, arguments or environment values.

**Reset to default preset** in Settings replaces `routing.json` and `defaults.json` together. It requires a confirmation that names the overwrite, and the machine action re-reads the combined revision of both files before confirmation and checks it again under the owner lock, so a concurrent change after that observation is refused. The machine wrapper ignores a caller-supplied revision; it does not enforce the age of the Settings card's earlier read. Before writing, it keeps a private owner-only backup of each existing file, named `<name>.json.cortex-backup-<time_ns>` in the same configuration directory. Reset overwrites all routing rules, the coordinator default, the language and the owner ceiling. Project and `$CORTEX_ROUTING` overrides are not changed. If an ordinary pre-replacement write fails, completed replacements are rolled back; uncertain durability is reported without replay. A hard process kill between the two replacements can leave the pair mixed; the backups allow manual restoration.

## Defaults and policy ceiling

`defaults.json` is created under the user config root when it is absent (see [Default preset](#default-preset)). The supported top-level keys are `defaults` and optional `ceiling`; when missing, defaults are empty and the ceiling is unrestricted. This file is persisted by the route-selection/configuration flow; manual edits should be cautious because the route identifiers are derived from observed engine catalogs.

A route default has this shape:

```json
{
  "defaults": {
    "route_id": "xr_<observed-route-id>",
    "route_connection_id": "cn_<configured-connection-id>",
    "route_model": "gpt-5.6-luna",
    "parameters": {"thought_level": "medium"}
  }
}
```

The model, connection ID and parameters must correspond to an observed selectable route. Parameter values such as `thought_level`, `effort`, `mode`, `model_config` and `summarizer` are accepted only when the selected route's observed catalog advertises that exact name/value pair. Do not copy a made-up route ID or assume all engines accept the same parameters. Configuration selection is revalidated by the runtime and `ConfigurationService`. When a session starts, the selected effort (`thought_level` or `effort`) is sent under the effort option the engine session itself advertises: `reasoning_effort` for codex-acp and `effort` for claude-agent-acp. Both use the `thought_level` category. A value the session does not offer is refused before any engine call.

The optional `ceiling` object may only narrow permissions and route access. Its exact fields are:

```json
{
  "allowed_routes": null,
  "allowed_connections": ["cn_<configured-connection-id>"],
  "allowed_data_recipients": null,
  "permission_level": "STANDARD",
  "limits": {"attempts": 8},
  "spend_limit": null,
  "spend_limit_enforceable": false,
  "fallback_policy": {
    "allowed_target_routes": null,
    "allowed_failure_classes": ["engine_failure"],
    "budget": 0,
    "data_recipient_consent": false
  }
}
```

- Route/connection/recipient fields are arrays of strings or null. Null means unrestricted at that policy layer.
- `permission_level` is null or `READ_ONLY`, `STANDARD`, or `ELEVATED`.
- `limits` is an object of named integer limits. Only `attempts` is enforced for routing attempts currently; `calls` and `tokens` are not metered by this code.
- `spend_limit` is a number or null; `spend_limit_enforceable` is a boolean.
- Fallback route/failure lists are arrays of strings or null; `budget` is an integer and `data_recipient_consent` is boolean. Fallback remains disabled without the required policy and any required per-recipient consent record.

The accepted ceiling model is `PolicyCeiling`; serialization/loading is `configuration.store.serialize_ceiling/deserialize_ceiling/read_user_defaults`. A project ceiling is stored in the project's SQLite configuration table and may further restrict the user ceiling. There is no second project JSON file for defaults. Task and assignment configuration can narrow the effective ceiling but cannot grant more.

Access mode is a separate client-owned choice, not a user-default JSON option: fresh ACP sessions durably start Development, with host-writable brokered commands for writing workers. Guarded keeps workers inside the project with consent for every outside action; Auto uses the same host-writable commands as Development and answers every permission card itself. Existing sessions/tasks without initialization and unknown/unreadable state remain Guarded. Pre-task choices persist per session and win at atomic task binding; reconnect/load preserves them without rewriting owner settings. The additive session record and task events live in the project database. See [permissions and limits](features/permissions-and-containment/index.md#access-mode-development-guarded-and-auto).

ACP's first pending/saved-default selection and later route/effort selections
read the owner defaults/ceiling and project SQLite layer. Only parameter maps
enter engine configuration; saved route identity, coordinator preference,
language and ceiling envelopes are not native parameters. The effective ceiling
is retained through selected-to-active application. A subsequent selection
intersects supplied layers with the prior task ceiling and retains an omitted
role profile, so omission or a route change cannot widen task grants.

Before a task starts, echoing the already selected exact model/provider is
idempotent: absent native parameters are not a request to clear them. It keeps
the saved portable coordinator and configured effort/mode unchanged. An actual
route change still resets dependent parameters; an explicit effort selection
still changes that field. No ACP reset signal or catalog-order inference is
introduced. Configuration states and engine confirmation references, not just UI values, are the evidence (the former bound inspector was removed; see [removed readers](features/observability/index.md#removed-python-era-readers)).

For automatic worker/reviewer routing, `allowed_routes`, `allowed_connections`
and `allowed_data_recipients` (connection provider IDs) filter the observed
candidates before ranking. Null/absent sets are unrestricted at that layer;
empty arrays permit nothing. A pin outside the intersection refuses without
substitution. Dispatch rechecks the chosen current route identity, not the
coordinator's route; an unavailable or ambiguous identity fails closed.

## Routing rules

The routing loader reads eight keys: `pin`, `deny`, `enabled`, `auto_allow`, `tiers`, `providers`, `engine_affinity` and `models`; additional JSON keys are currently ignored. Its validation and selection logic are in `auto_route` and `model_policy`. These are the eight supported routing keys (unknown keys are ignored):

```json
{
  "pin": {
    "worker-security-auditor": "claude-agent-acp/opus:high",
    "*": "codex-acp/gpt-5.6-luna"
  },
  "deny": ["gpt-5.5", "haiku"],
  "enabled": {"opencode": ["anthropic/claude-sonnet-4-5", "nvidia/nemotron-ultra"]},
  "auto_allow": ["claude-agent-acp/haiku"],
  "tiers": {
    "gpt-5.6-luna": "light",
    "gpt-5.6-sol": "standard"
  },
  "providers": ["codex-acp", "claude-agent-acp"],
  "engine_affinity": {
    "implementation_backend": "claude-agent-acp",
    "testing": null,
    "*": "codex-acp"
  },
  "models": {
    "opencode": {
      "auto": ["anthropic/claude-sonnet-4-5"],
      "tiers": {"anthropic/claude-sonnet-4-5": "standard"},
      "policy": {"anthropic/claude-sonnet-4-5": {"preferred_for": ["review"]}}
    },
    "codex-acp": {
      "policy": {"gpt-6-sol": {"preferred_for": ["design"], "max_effort": "xhigh"}}
    }
  }
}
```

Routing validation diagnostics use generic `opencode/provider/model` (pin) and
`provider/model` (enabled model) syntax, not a fixed model recommendation. They
still require exact provider-qualified IDs and reject invalid settings before
writing configuration or dispatching work.

- `pin`: object mapping worker role names or `*` to `engine/model` or `engine/model:effort`. A pin is strict: unavailable or disallowed pins fail instead of silently switching routes.
- `deny`: array of model tokens, each optionally prefixed `engine/`. A token is an exact model id; a versioned id, which also covers that version's variants (`gpt-5.5` covers `gpt-5.5[1m]`, not `gpt-5.6-luna`); or a bare family name, which covers every version and alias of that family (`haiku` covers `haiku`, `claude-haiku-4-5` and `claude-haiku-4-5-20251001`). A token with a `[...]` variant is exact only. The special `<supported-engine>/*` token disables every model for that engine, including future models. Explicit deny applies to automatic routing, pins, dispatch, and the model selector, and always wins; it does not unregister the connection.
- `enabled`: object mapping a supported engine id to an array of exact model ids (OpenCode ids are `<provider>/<model>`). An engine it lists may use only those models in Cortex; every other model of that engine is treated exactly like a denied one (hidden from the selector, refused when chosen or pinned, never routed automatically), and `[]` switches all of its models off. An engine it does not list keeps the built-in default: every model is on, except the optional OpenCode engine, whose models stay off until listed here (OpenCode offers hundreds of provider models). `deny` still wins. Settings > Models writes this list for OpenCode and exact `deny` entries for the default-on engines.
- `auto_allow`: tokens of the same form that may enter automatic selection despite the built-in OpenCode exclusion (on an engine without a `models` selection list; a list replaces this exclusion). OpenCode model IDs name their provider, so an OpenCode entry is the exact route, for example `"opencode/anthropic/claude-sonnet-4-5"`. It does not override `deny`, `providers`, catalog availability, connection readiness or quota limits.
- `tiers`: object mapping model names to `flagship`, `deep`, `standard`, or `light`, for every engine. Built-in known families/models are classified automatically; this key overrides that classification, and a per-engine `models` tier overrides this key. A Luna model moved out of `light` routes like any model of its new tier; only the Luna effort floor (`high`) and `max` still apply.
- `providers`: array restricted to `codex-acp`, `claude-agent-acp` and/or `opencode`. When absent, every configured provider is considered. It restricts automatic routing, failover and pins; it does not hide models from the selector or disable manual selection. A disallowed pin is refused.
- `engine_affinity`: object mapping a work kind or `*` to `codex-acp`, `claude-agent-acp`, `opencode`, or `null` (no lean). Work kinds are `implementation_backend`, `implementation_frontend`, `implementation_frontend_design`, `operations`, `debugging`, `testing`, `investigation`, `review`, `design`, `writing`, `planning`, and `analysis` (each worker role's kind is `ROLE_WORK_KINDS`; an unlisted role is `implementation_backend`). A kind named here wins, then `*`, then the built-in default: `implementation_backend`, `implementation_frontend` and `operations` lean to `codex-acp`; `implementation_frontend_design`, `investigation`, `review`, `design`, `writing`, `planning` and `analysis` lean to `claude-agent-acp`; `debugging` and `testing` are neutral. Affinity breaks ties only after the 95% quota band and retry diversification (rank version 15): a known route at or above 95% ranks behind any non-hot route, so affinity never overrides that band. Plan usage is only the last tie-breaker after affinity (see [usage switching](#explicit-user-model-priority-and-usage-switching)). Affinity never overrides a pin, `deny`, `providers`, availability or the 100% quota exclusion. An unknown kind or engine makes the file invalid.
- `models`: object mapping a supported engine id to an object with any of `auto`, `tiers` and `policy`, each keyed by the exact model id the engine reports (OpenCode ids are `<provider>/<model>`). Validation reports every problem with its field path.
  - `auto`: array of unique model ids. Present, it is the complete list of that engine's models automatic routing may use (`[]`: none), replacing the built-in OpenCode exclusion, `auto_allow` and newest-version-only routing for that engine; absent, the built-in rules apply. `deny` and `providers` still win; a `pin` or the model selector can still use any offered model.
  - `tiers`: model id to `flagship`, `deep`, `standard` or `light`, or to a non-empty list of them (for example `["standard", "deep"]`); it wins over the legacy top-level `tiers`, which wins over the built-in classification. A model given several tiers is an automatic-routing candidate of each (it is listed under each in `list_route_options`) and runs at the effort of the tier it was chosen for; a pin or explicit effort may use any effort one of its tiers permits, and it counts as flagship for a user's explicit flagship request when `flagship` is one of its tiers. Only this per-engine key takes a list; the legacy top-level `tiers` stays one tier per model. In Settings > Engines & models the tier control is a multi-select; clearing every tier removes the setting.
  - `policy`: model id to an object of optional attributes, each overriding the model's built-in value (`null` removes it): `effort_floor` and `max_effort` (efforts `low`..`max`; without `max_effort` a flagship-tier model stops at `high`), `effort_ladder` (1-3 strictly ascending efforts a light model runs at for light work, standard work, and standard work that fell short once), `also_standard_for` (work kinds for which a light model is also a standard-tier candidate, only for explicitly configured work kinds), `preferred_for` (work kinds this model may do: a model that lists kinds is never a candidate for a node of another kind, including on retries and escalation, which choose the senior model among the eligible ones; a model without listed kinds serves every kind, and when no usable model lists a node's kind every usable model stays eligible so work never stops. Among eligible models it is also preferred: rank version 9 puts it ahead of new plan hints, plan usage and retry diversification within its tier; it yields for quota only when at least 95% used and more than 10 points worse than a known usable alternative; plan usage compares only the preferred models while they are honoured, and none once they are used up), `preferred_when` (optional owner-written condition, non-blank text of at most 500 characters, for example `"frontend changes without design work"`; see below) and `request_names` (1-16 lowercase extra spellings, Cyrillic stems taking Russian endings, that count as the user naming this model's family in an explicit flagship request; ordinary request, work or negation words are refused).

  `preferred_when` makes the preference conditional. The runtime cannot judge
  free text, so `list_route_options.preferred_when` gives the coordinator each
  such auto-eligible route with its `when` text and `work_kinds` (the model's
  `preferred_for`, empty meaning any kind); such routes are left out of
  `preferred`. The model ranks as preferred (rank key `purpose`) only on a node
  whose plan hint names its exact `engine/model` route, which is the
  coordinator's judgement that the node meets the condition, and only for a
  listed kind when `preferred_for` is non-empty. On other nodes it is an
  ordinary candidate of its tier; retries and fixes ignore hints and so the
  condition too. Like `preferred_for`, a non-empty `preferred_when` needs the
  model in an explicit `models.auto` list. In Settings both work-kind lists
  are chosen from the work-kind vocabulary, never typed, and the condition
  is a text field next to the preferred-work switches.

  `preferred_for` uses the same twelve work kinds as `engine_affinity`. The API's
  the runtime includes English `work_kind_labels`, the worker-role list
  `work_kind_roles`, and `builtin_affinity` for each kind. A model with a
  non-empty `preferred_for` must appear in its engine's explicit `models.auto`
  list when that list exists (`preferred_not_selected` otherwise). Without a
  list, built-in eligibility is dynamic: the route catalog marks a preference
  `preferred_inactive` when its route is not auto-eligible, and
  `list_route_options.preferred` includes only auto-eligible routes. The latter
  also returns `affinity_source` (`routing.json` or `builtin`) per kind;
  `affinity` is `null` when that engine has no auto-eligible route, with the
  withheld engine named in `affinity_not_eligible`.

  The coordinator sees the effective selection, tiers and policy only through `list_route_options`; no prompt or tool schema names a model. See [routing](features/routing-and-fallback/index.md#owner-configured-models-selection-tiers-and-policy).

Precedence is first existing routing file: project `<project>/.cortex/routing.json`, then `CORTEX_ROUTING`, then user `routing.json`. The files do not merge, so a project file that should keep the defaults must list them itself. An existing malformed file fails closed. If no file exists, built-in automatic routing applies.

### User routing profiles

The `routing_profiles` management surface stores owner-only JSON at
`$XDG_CONFIG_HOME/cortex/routing-profiles.json`. Format 2 stores a profile's
`sections` and settings snapshot: the eight supported user-routing keys;
user defaults (route id, connection id and exact model id, named parameters,
language, and owner policy ceiling); each registered MCP server's usage
policy; and each registered connection's concurrent-session limit. The profile
file is a management snapshot, not a runtime routing source. Activation
materializes settings into the existing backing files; their loaders and
precedence remain unchanged.

Profiles exclude project routing and project ceilings, MCP server installation
definitions (`command`, `args`, `env`), connection credentials or credential
paths, engine login state and task state. Credentials remain in their existing
private stores and are neither copied nor exposed by profile reads. Names are
lowercase letters, digits or hyphens, 1–40 characters; at most 32 profiles.

Choose the current profile in the persistent profile bar. Apply the forms in
each settings section first, then use **Save to current profile** (`save`) to
replace that profile's captured settings, or **Save as new profile** (`create`)
to capture the current settings and select the new profile. A normal settings
save updates working settings only; it never writes through to the selected
profile. `update` changes profile name/description only. Activation restores
the saved snapshot to user backing files after validating all targets; a
journal is written before ordered file replacement, with in-process rollback
if a write fails. Switching away from modified settings offers save, discard
and switch, or cancel. Discarding requires the explicit
`discard_modifications: true`; `deactivate` detaches without changing working
settings. Re-activating an interrupted profile retries its pending activation;
deactivation clears the pending operation. The UI refreshes the mounted form
after activation.

Profile states are `none`, `in_sync`, `modified`, `interrupted` and
`unavailable`. `modified` means working values differ from the selected
snapshot; section names and key-only differences are reported without values.
`interrupted` indicates a pending journal remains after a failed restore;
`unavailable` means the profile or a required working section could not be
read safely. A partial legacy profile can be explicitly upgraded by saving a
complete snapshot. Every mutation requires the profile file's latest
`expected_revision`; create, save and activate also require
`expected_revisions` for all four sections from `read.current_revisions`.
Stale revisions refuse without writing. Reads do not write.

Format 1 is interpreted in memory as a routing-only profile. Its first
mutation creates a private verified backup under
`$XDG_CONFIG_HOME/cortex/.migration-backups/` and migrates the file to format
2; profile content remains routing-only until explicitly saved as a full
snapshot. Unknown formats fail closed.

Project routing or `$CORTEX_ROUTING` still wins over materialized user
routing. `shadowed_by` reports the winning source; selecting a profile cannot
force worker selection through a higher-precedence file or change an already
open worker's route. The route audit labels the profile only when its routing
matches the winning user file. A preference in an owner profile is ranking
input, not permission to dispatch.

#### Settings reference

“Level” identifies which configuration surface owns the setting. “Profile” says
whether the owner-profile snapshot captures it. Examples are illustrative values,
not a universal preset; exact model/connection identifiers must exist locally.

| Setting | What it changes | Level | Profile-bound? | Example |
|---|---|---|---|---|
| Default route (engine, connection, exact model) | Route used as the user default for new work when no more specific choice applies | User defaults | Yes | `codex-acp`, `cn_0123456789ab`, `gpt-5.6-luna` |
| Default language | Language preference for Cortex responses | User defaults | Yes | `en` |
| Named parameters | Default runtime parameters associated with work | User defaults | Yes | `effort=high` |
| Owner policy ceiling | Widest owner-level allowed routes, recipients, permission level, limits, spend/fallback policy | User policy | Yes | `permission_level: "STANDARD"` |
| Project policy ceiling | Narrows the owner ceiling for one project | Project | No | `permission_level: "READ_ONLY"` |
| Pin | Requires an eligible exact engine/model/effort route for a worker role | User or project routing | User: yes; project: no | `{"implementation":"codex-acp/gpt-5.6-luna"}` |
| Deny rules / whole-engine switch | Refuses routes and hides denied models; `<engine>/*` disables that engine in Cortex | User or project routing | User: yes; project: no | `["gpt-5.5"]` |
| Model enabled switches | Selects models available in Cortex's selector and dispatch; optional OpenCode models default off | User or project routing | User: yes; project: no | `{"opencode":["provider/model-id"]}` |
| Auto-allow | Opts an otherwise excluded exact route (for example an OpenCode route) into automatic selection, subject to remaining policy | User or project routing | User: yes; project: no | `["opencode/anthropic/claude-sonnet-4-5"]` |
| Model tiers | Assigns automatic-routing candidates to light, standard, deep or flagship tiers | User or project routing | User: yes; project: no | `{"gpt-5.6-luna":"light"}` |
| Automatic-routing providers | Restricts providers considered for automatic routing, failover and pins | User or project routing | User: yes; project: no | `["opencode","claude-agent-acp"]` |
| Engine affinity | Breaks remaining routing ties toward an engine for a work kind | User or project routing | User: yes; project: no | `{"implementation_backend":"opencode"}` |
| Per-model automatic selection | Defines the full set of models an engine may automatically route when `models.<engine>.auto` is present | User or project routing | User: yes; project: no | `{"opencode":{"auto":["provider/model-id"]}}` |
| Per-model policy and work-kind preference | Sets effort/tier behavior and preferred work kinds for an exact model | User or project routing | User: yes; project: no | `{"opencode":{"policy":{"provider/model-id":{"preferred_for":["review"]}}}}` |
| MCP enabled and usage limits | Controls whether a registered server and its tools, paths, networks, roles or engines may be used | User MCP policy | Yes | `enabled: true` |
| MCP command, arguments and environment | Defines how an MCP server is installed/launched and its environment | User MCP definition | No | `command: "node"` |
| Connection session limit | Caps concurrent sessions on one registered connection | User connection policy | Yes | `max_concurrent_sessions: 2` |
| Connection name and onboarding | Changes display metadata or explicitly opts into connection setup | User connection metadata | No | `name: "Work"` |
| Connection credentials and login state | Authenticates to a provider; stored/referenced separately from profile data | User secret/engine state | No | Existing native login |

For routing, user values are profile-bound only when editing user routing; the
same keys written to project routing remain project-only. For MCP, the usage
policy is captured but server installation definitions are not. For connections,
only the concurrency limit is captured—not identity, name, credentials or login.
The screen's inline help describes each control's effect and scope; this table
summarizes the underlying settings rather than every repeated row in a dynamic
list.

In the default Rust ACP entry, `/cortex-new-task` resolves the default route (a
session's pending route choice applies only to that session's own task) and refuses
before creating anything when no route resolves. `/harvest` and `/harvest-refresh`
word their request in the session language; a pre-task language choice is also
saved as the default language, as in Python. See
[task commands](project/native-incoming-acp.md#task-commands).


## MCP servers for Cortex and workers

Cortex may pass session MCP entries supplied by the ACP client, and it can load owner-managed stdio servers from `$XDG_CONFIG_HOME/cortex/mcp-servers.json`. A project-local MCP registry is deliberately unsupported: changing project contents must not install a persistent executable capability. The loader is `load_mcp_servers`; setup and engine sessions validate explicit tool grants and containment.

An owner entry whose `command`, or (for a contained entry) any `read_paths`/`write_paths` value, does not exist on the host is not forwarded to engine sessions: the checkout drops it and logs `external_mcp_server_path_missing`. A deployed package that predates this filter forwards it, and the client then sees the server close at once (`CONNECTION_CLOSED`). Correct the path or refresh the deployed package; see [browser and Playwright status](features/design-workflow/index.md#browser-and-playwright-status).

Example (replace executable, tool names and paths with values appropriate to your installed server):

```json
{
  "version": 1,
  "servers": {
    "example-index": {
      "command": "/opt/example-index/bin/server",
      "args": ["--stdio"],
      "env": {"INDEX_CACHE": "/home/alex/.cache/example-index"},
      "allowed_tools": ["search", "read_result"],
      "read_only_tools": ["search", "read_result"],
      "coordinator_tools": ["search"],
      "read_paths": ["/home/alex/projects"],
      "write_paths": ["/home/alex/.cache/example-index"],
      "network": false,
      "runtime_dir_env": ["EXAMPLE_RUNTIME_DIR"],
      "roles": ["coordinator", "worker"],
      "engines": ["codex-acp", "claude-agent-acp"]
    }
  }
}
```

Fields:

- Root `version`: integer, exactly `1`; `servers`: object keyed by server name. The loader also accepts `mcpServers` as an alias for the server map.
- Root `containment`: optional registry default for entries without their own `containment`, `"contained"` (the default when absent) or `"none"`.
- `containment`: optional, `"contained"` or `"none"`; overrides the root default. Any other value, at either level, makes the registry invalid. `"none"` runs the server without the Cortex sandbox, with the owner's full user authority: no Landlock, seccomp or network restriction. `read_paths`, `write_paths`, `network` and `runtime_dir_env` are then ignored (a missing grant path no longer drops the server), and the server gets the owner's `HOME`, a `PATH` of its executable's directory, `~/.local/bin`, `~/bin`, `/usr/local/bin`, `/usr/bin` and `/bin`, and the Cortex process's `XDG_RUNTIME_DIR`. Tool grants and the relay still apply, and Cortex stops the server's process group when the session ends; a daemon that detaches from that group is not stopped by Cortex. Use it only for a server you trust like any program you run yourself, typically one that cannot start inside the sandbox. It is machine-local: profiles neither capture nor replace it. The Settings switch **Run without the sandbox** writes it per server, omitting it when it equals the registry default.
- Required `command`: non-empty executable string; `args`: optional string array, default `[]`.
- Optional `enabled`: boolean; `false` skips this registry entry. When omitted it defaults to true.
- `env`: optional environment object or array of `{"name":"KEY","value":"VALUE"}` objects; names and values must be strings without NUL. Reserved launch variables cannot be overridden. Do not put real credentials in this file; protect this owner-only registry and prefer a server's supported external secret source.
- Required `allowed_tools`: non-empty list of exact tool names granted to Cortex. Unknown or omitted tools are not implicitly granted.
- `read_only_tools`: optional array, default `[]`, subset of `allowed_tools`.
- `coordinator_tools`: optional array, subset of `read_only_tools`; use this to limit which read-only tools the coordinator receives.
- `read_paths`, `write_paths`: optional arrays of absolute paths, default `[]`. Writes are limited to explicit grants and the engine jail's scope. A writing worker's scope path that does not exist yet when its session starts is not granted to the server.
- `network`: optional boolean, default `false`.
- `runtime_dir_env`: optional unique array of environment-variable names. The runtime provides a private per-server directory through these names. `PATH`, `HOME`, `TMPDIR`, and `XDG_RUNTIME_DIR` are reserved.
- `always_load_tools`: optional array, subset of `allowed_tools`. The gateway marks these tools with MCP `_meta` `"anthropic/alwaysLoad": true` in `tools/list`, so Claude keeps their schemas in the prompt instead of deferring them behind `ToolSearch`. Codex ignores the mark. Each listed schema costs prompt context in every Claude session that receives the tool.
- `roles`: optional non-empty unique subset of `coordinator`, `worker`.
- `engines`: optional non-empty unique subset of `codex-acp`, `claude-agent-acp`.

Cortex-owned entries take precedence over client-supplied MCP entries with the same name; other client entries pass through for that session. MCP server values are loaded into memory for engine sessions, not copied into task reports. External tools still pass the gateway's channel, role, tool, path and network checks. The gateway forwards each external `tools/list` schema unchanged except that it adds a description to an argument the server leaves undescribed when Cortex knows its semantics (currently `codebase-memory-mcp` `search_graph` `file_pattern`, `name_pattern` and `qn_pattern`; see [Codex adapter](features/codex-engine/index.md#mcp-bridge)); it never replaces a description the server supplies. Claude defers MCP tool schemas behind its native `ToolSearch`. Cortex marks every tool of its own `cortex` server channel catalogue as always loaded, and external tools only when listed in `always_load_tools`. Every engine session that receives external servers also gets an "Attached MCP servers" section after its role instructions naming them (see [unified system prompt](features/unified-system-prompt/index.md)). The full `codebase-memory-mcp` procedure appears only when that role is granted `list_projects`, `search_graph`, `trace_path` and `get_code_snippet`; a partial grant yields a limited-capability note instead. For coordinators, include those tools and `index_status`/`check_index_coverage` in `coordinator_tools` if the full lookup procedure is intended; keep `index_repository` worker-only. List read-only index tools in `always_load_tools` so Claude need not discover their schemas through `ToolSearch`. Read [permissions and containment](features/permissions-and-containment/index.md) for the enforcement boundary.

The additive Rust native host reads the same owner-global registry and grant fields.
Its incoming client accepts configured names only; unlike the retained adapter's
session fallback, an unconfigured client entry is refused. Native containment uses
Landlock/seccomp with narrower local-socket and project-scope support, not Python's
namespace launcher. See [native external MCP boundaries](project/native-incoming-acp.md#external-mcp-boundary)
before selecting that optional entry point; this does not change installed clients.

## ssh from workers

ssh in a worker's native shell and in `command_run` behaves as it does in your terminal. HOME is your own, read-only, so `~/.ssh/config`, `known_hosts` and host aliases apply. Cortex passes your running ssh-agent (`SSH_AUTH_SOCK` from the environment Cortex was started with) to every engine process and command, provided it names a socket you own. Keys unlocked in that agent, such as a desktop keyring's, work without a passphrase prompt. System ssh configuration that root owns, such as `/etc/ssh/ssh_config.d/*.conf`, is accepted as on the host even though the sandbox shows root's files as `nobody`'s (see [SECURITY.md](../SECURITY.md#owners-ssh-agent)). Nothing needs configuring. Without an agent, a key with a passphrase needs the optional password dialog below.

For prompts the agent cannot answer (a key passphrase, a password, a new host-key confirmation), `{"enabled": true}` in `$XDG_CONFIG_HOME/cortex/ssh-askpass.json` shows ssh's own prompt in a desktop dialog. See [SECURITY.md](../SECURITY.md#owners-ssh-agent) and `ssh_askpass`. Both apply to engine processes started after the change, so open a new session.

## Temporary files and engine updates

Cortex keeps its temporary files in `/var/tmp`, which is on disk. On many hosts `/tmp` is a tmpfs, which lives in
RAM and swap. Every entry point points `TMPDIR` at `/var/tmp` when it is unset or is `/tmp`
itself. An explicit `TMPDIR` is kept: a sandbox's own scratch, an isolated
test run, or your own choice. Child processes inherit it.

At every start Cortex installs, in the background through npm, each npm
engine's manifest-pinned adapter (`pinned-dependencies.json`
`pinned_package_version`) when it is not installed yet: the same current
qualified version **Download engine** installs. npm's latest unqualified release
is never downloaded. The same background pass connects native accounts with login markers and loads model lists. `CORTEX_ENGINE_UPDATE=off` skips the entire automatic setup pass, which suits an offline
machine. The no-paid smoke harness and the test suite set it, so their
disposable HOME never gets an npm download (about 625 MB per launch). The smoke
harness deletes its temporary project and HOME after each run; pass
`--keep-temp` to keep them. Each test process works in its own `/tmp/cxpt-*`
directory and deletes it when it ends. Tests stay under `/tmp` because the
sandbox tests rely on the shared, writable `/tmp`, while a sandbox's
`/var/tmp` is private.

## Verify configuration

Check the current user registry and engine discovery without printing secret values:

```bash
scripts/cortex-paseo-tools.sh status
```

The former offline diagnostics report (`cortex_diagnose`) and the Python loader examples were removed on 2026-11-16 without replacement. To check configuration without a model call, use the read-only `status` above, or read the same data through the settings API (`cortex-paseo-tools config`, one JSON request on stdin) under isolated `HOME`, `XDG_CONFIG_HOME` and `XDG_STATE_HOME` outside every repository, with owner-only JSON files under `$XDG_CONFIG_HOME/cortex`. For routing, `CORTEX_ROUTING` may name a JSON file containing the documented routing shape. A read creates neither example registry files nor the project store.


## Settings-only model policy

Explicit model requests match the complete, case-sensitive configured
`engine/model` identity, not a family, substring or normalized variant. Keep
the identity whitespace-delimited, or quote it with paired `"…"`, `'…'`,
backticks, `«…»` or `“…”`. A closing quote may be followed by one prose
punctuation character (`. , ! ? ; :`) before whitespace/end. Without quotes,
attached punctuation is opaque identity data, even a terminal dot; use
`Use "engine/unit".` rather than ambiguous `Use engine/unit.`. Longer/shorter
variants, punctuated prefixes and different provider paths do not authorize
each other. Optional owner-configured `request_names` remain separate labels
with the existing case-insensitive ending rules and ambiguity checks. Retained
user-source provenance and request/negation grammar still govern authorization;
an unconfirmed plan hint alone never promotes a flagship or overrides hard routing authorization.
See [exact user-request boundaries](features/routing-and-fallback/index.md#settings-only-routing).

The runtime and coordinator use only the effective first-existing routing file (project, then $CORTEX_ROUTING, then user); these files are not merged. Routing must declare model_policy_version: 2. The read-only `list_route_options` tool preserves `model_policy_migration_required`, `model_policy_version_unsupported` and `invalid_routing_overrides`, with the policy field and corrective action; it does not misclassify these refusals as `storage_error`. Missing, invalid, unsupported or legacy settings produce visible diagnostics; runtime does not overlay, infer or repair them from install data. The effective policy explicitly supplies exact enabled identities, per-engine automatic membership, tier placement and any non-neutral per-model effort/preference/request-label attributes. Omitted membership means none; new, renamed, unmatched and withdrawn identities do not inherit policy from spelling, family, catalog position or neighboring versions. Required fields/tiers and supported effort constraints are validated; unsupported policy refuses rather than silently weakening it. Coordinator route choice is a portable exact engine/model/parameter preference resolved only against one unique current selectable catalog match; automatic-worker `providers` does not restrict that binding, while enabled identities and deny rules still do; ambiguous/unavailable matches remain diagnostics, not substitutions.

The validated public asset policy/model-settings-default.json is installation/migration input only, not a runtime fallback or overlay. It contains a production-derived baseline candidate and portable coordinator preference, but no account/connection identity or credentials. Setup, the Settings Apply action and each agent start validate it completely and create missing owner settings privately without replacement (see [Default preset](#default-preset)); existing, partial, invalid and legacy owner files and saved profiles are not silently changed. The asset cannot prove installed deployment or a live route. Migration is an explicit two-step operation: preview a candidate (non-mutating), then apply only with explicit consent, the exact preview source revision and candidate digest. Apply verifies a private backup and written bytes and rolls back on failure. Migrate each saved profile from a candidate derived from that profile itself; never fill an inactive profile from current settings or the install template. Retain backups and preview/apply evidence. See [model-policy migration](project/packaging.md#model-policy-installation-and-explicit-migration).


### Implementation work classification

The optional `plan_submit.plan.tasks[].work_kind` enum overrides the role's
classification without changing its tier or authorization:
- `implementation_backend`: backend-dev, data-engineer, refactorer, fullstack-dev,
  general and unlisted-role default.
- `implementation_frontend`: frontend-dev, mobile-dev and accessibility-fixer
  default, for frontend implementation without visual design.
- `implementation_frontend_design`: explicitly classify layout, styling, markup,
  responsive layout, UI components and visual polish this way, including those
  changes implemented by fullstack or accessibility specialists.

The runtime validates the enum, retains it in the accepted plan JSON, and uses it
for affinity, preferences and light-model standard allowances at dispatch.
Retries and review fixes inherit the original explicit classification.
`implementation_frontend_design` is affinity-bound: automatic attempts prefer
its effective `engine_affinity` engine (Claude by default), and preserve
`preferred_for` selection across retries, escalation and eligible fallback tiers,
without requiring a conditional model hint. Only when that engine has no
eligible route at all does selection widen to the normal cross-engine
candidates, never while the affinity engine still has one eligible route.
No route anywhere, or a neutral affinity, refuses visibly with
`no_available_route`; a cross-engine pin refuses with `pinned_route_unavailable`
regardless of fallback. Other kinds retain soft affinity.
Confirmed explicit user model requests still take precedence, without bypassing
hard authorization or availability. Without
an override, a fix uses its actual writing role's default. `list_route_options`
exposes all three kinds in affinity and preference metadata; `role_work_kinds`
continues to describe defaults, not per-node overrides.

Version-2 routing files and older profiles may still use `implementation` in
`preferred_for`, `also_standard_for` and `engine_affinity`. Runtime and settings
effective reads expand it to all three kinds in memory, deduplicating mixed
legacy/new lists. An explicit new-kind affinity wins over the legacy umbrella,
then the umbrella wins over `*`. Per-source `rules` retain the stored spellings for
editing; only `effective_rules` expand them. An unrelated Settings save preserves
legacy affinity, preference and stretch entries rather than migrating them.
Reads never rewrite the source file or profile; the policy version remains 2. The legacy spelling is not a valid new plan override.

### Fresh-install owner default snapshot

The bundled preset lists Haiku, Luna, Sonnet, Sol and Terra as preferred for
every work kind except frontend design implementation, whose Opus preference
is unchanged. Preferences rank eligible models, not the tiers themselves.
Haiku and Luna serve light work; Sonnet, Sol and Terra serve standard work;
Sonnet and Sol serve deep work alongside conditionally preferred Opus.
Haiku and Luna no longer count as standard candidates for testing. Normal
testing uses the standard pool when available; fallback rules still apply.

Planner, general and technical-writer roles use light, standard and deep for
low, normal and high complexity respectively. Explorer uses light for low and
normal, and standard for high. Deep routing targets high effort, subject to
model-specific constraints. The portable coordinator is `claude-agent-acp/opus`
at medium effort.

The public install asset is based on the saved production `default` profile from
2026-10-12 at 14:23 UTC, with the implementation-kind and tier refinements below. Its routing is a non-secret snapshot, including
multi-tier memberships, effort floors/ceilings, preferences, request labels and
quota-window associations. The portable coordinator is
`claude-agent-acp/opus` with `thought_level: medium`; no mode override is
added. New `defaults.json` files also set `language: ru`. The install envelope
remains schema version 1 with optional `language` (`en` or `ru`) and
`mcp_servers` (disabled installation examples); coordinator
`mode` is optional, and when present the install template accepts `agent`.

This is portable model/settings installation, not cloning the owner's machine:
route/connection IDs, account bindings and real local paths are excluded. MCP
installation definitions use only disabled example paths and empty environment maps. The saved unrestricted ceiling needs no override.
Existing connection session limits and MCP registrations/policies are not changed;
existing saved `routing-profiles.json` snapshots are never replaced. Fresh installs
create the applied `default` profile and disabled example MCP registrations as described
in [Default preset](#default-preset). A fresh host must configure its own MCP executables
and native engine accounts and resolve the exact coordinator against
one observed selectable route. Missing or unsupported model/effort observations
still fail closed; this snapshot is not live qualification.

The preset makes `gpt-6.2-astra` flagship-only, never a deep-tier automatic candidate.
Only `opus` lists `implementation_frontend_design` in `preferred_for` in the bundled preset.
Synced from the owner's 2026-10-20 production routing update, Opus's
`preferred_for` had narrowed to `design`, `planning`,
`implementation_frontend_design`, `debugging` and `analysis`, dropping
`implementation_backend`, `implementation_frontend` and `review`; Sonnet-5 also
gained the `deep` tier alongside `standard` in that sync.

Synced again from the owner's 2026-10-21 production routing update, Opus's
configured tiers drop `standard` entirely (now only `flagship` and `deep`),
and `preferred_for` regains `implementation_backend` and
`implementation_frontend` (now `design`, `planning`,
`implementation_frontend_design`, `debugging`, `analysis`,
`implementation_backend`, `implementation_frontend`). Because Opus no longer
configures the `standard` tier at all, standard-tier work -- including plain
`design` and `implementation_frontend` -- never reaches Opus by purpose or by
an explicit hint: the hint is ignored on tier mismatch rather than ranked
against the owner's preference, and the owner's eligible Sonnet/Sol preferences
serve that tier instead. Opus still serves `design`, `implementation_backend` and
`implementation_frontend` through its remaining `deep`/`flagship` tiers once
work escalates or is otherwise eligible for those tiers. Sol's (`gpt-6.2-sol`)
`preferred_for` also gained `testing`, `investigation` and `writing` in that
sync; the 2026-10-27 priority refinement removes testing, debugging and writing again. The latest refresh removes Sol's free-text condition (`preferred_when: null`),
so its listed `preferred_for` kinds apply without a route hint, including on
quality retries. With equal quota and no provider preference, unhinted standard
`design` and `implementation_frontend` now select Sol rather than Sonnet;
quota pressure can still pass over that preference. The snapshot retains the
Opus UI/UX and Sonnet complex-backend conditions in `preferred_when`, independent
of the `preferred_for` enum. For explicit
`implementation_frontend_design`, structured `preferred_for` selection
applies independently of that text and survives quality retries/fixes: Opus
remains selected while eligible (its configured `deep`/`flagship` tiers) and
not passed over by the existing hot-and-meaningfully-worse quota rule.
Escalation cannot substitute a different engine or a non-preferred higher-tier
model while a preferred route remains usable in an eligible fallback tier.

If Opus is unconfigured, unavailable, automatically excluded, outside the task
ceiling or exhausted, another eligible route on the affinity engine (Claude in
the preset) serves this work first. Only once Claude has no eligible route at
all does dispatch fall back to the normal cross-engine selection -- Sol or
Astra on Codex in the preset -- instead of refusing; while Claude has even one
eligible route, Codex is never chosen for this work. If no route is eligible on
any configured engine, dispatch refuses visibly rather than inheriting the
coordinator. Pins remain strict; a cross-engine pin is refused regardless of
this fallback. Confirmed user model requests still precede automatic routing.
No authorization, tier/effort policy or quota-exhaustion rule is weakened.

For models retaining a non-null `preferred_when` (unlike Sol), other work kinds,
including the role-default `design` kind, still require the coordinator's exact
offered route hint to satisfy that condition, and ignore conditional hints on
quality escalation. The `request_names`
`opus`/`опус` still permit explicit user requests. OpenCode is explicitly denied
by `opencode/*` and `auto_allow` is empty; dormant OpenCode model policy remains
preserved. See [affinity-bound frontend design](features/routing-and-fallback/index.md#affinity-bound-frontend-design).

### Haiku 5.5 and Luna parity

The preset enables `claude-agent-acp/haiku`, observed as Haiku 5.5 with
SDK 0.3.293 / CLI 2.1.293. Cortex's updater installs that exact SDK refresh
beside upstream adapter 0.87.0 (whose SDK 0.3.287 still resolves the alias
to Haiku 4.5); see [adapter evidence](features/claude-engine/index.md#haiku-55-catalog-and-effort-evidence).
Do not treat an older install's `haiku` label as proof of the new model.
A Cortex runtime older than that build-name ordering repoints the connection
to the upstream install at every start; see
[catalog refresh lifetime and deployed runtimes](features/claude-engine/index.md#catalog-refresh-lifetime-and-deployed-runtimes).

Both `haiku` and `codex-acp/gpt-6-luna` are light-tier only, with no
`also_standard_for` testing override. Both have effort floor high, ladder
high/xhigh/max, maximum max, identical preferred
work kinds and null request names. The SDK supports all three chosen levels;
no unsupported effort was substituted. `seven_day_haiku` associates only
the exact new alias. The deny entry is the exact old
`claude-agent-acp/claude-haiku-4-5-20251001`, not bare `haiku`.
Deny matching remains exact; it is not a family rule.

The owner's production routing and selected `default` profile were
deliberately updated together after private backups; unrelated profile
sections stayed unchanged. This does not change setup's rule that existing
owner settings/profiles are never silently overwritten, nor project/env
precedence or routes of already-open sessions. No paid model turn or GUI
qualification was performed.

### Exact quota-window associations

Version-2 routing also accepts optional `quota_windows`: an engine id to an ordered list of `{ "window": <exact reported label>, "models": <null or list of exact model ids> }` rules. Configuration API version 1 accepts the same field through the settings API; the public installation envelope stays schema version 1. For example (this policy fragment does not enable any models):

```json
{
  "model_policy_version": 2,
  "quota_windows": {
    "codex-acp": [
      { "window": "owner_scope", "models": ["EXACT@9"] },
      { "window": "primary", "models": null }
    ]
  }
}
```

- `window` is case-sensitive, not trimmed or interpreted as a model family. SDK window labels are used exactly as reported. The Claude local usage parser still normalizes printed scope labels to lowercase `seven_day_` labels with whitespace replaced by underscores; this parses evidence, not model membership.
- `models: null` explicitly marks a shared/account window. A nonempty list associates that label with those exact models on that engine; `[]` explicitly excludes the window for every model. Multiple different applicable windows contribute their maximum open reported percentage, not a sum. Duplicate exact window labels on one engine are ambiguous and refused; duplicate model ids, unsupported engines, unknown rule fields or malformed values are also refused before quota IO. Limits are 1000 rules per engine, 1000 unique model ids per rule, and 256 characters per nonempty window label (no controls). OpenCode ids must include the exact provider/model.
- Missing `quota_windows`, `{}`, an omitted engine, an empty engine list and any **unmapped** reported label all use conservative shared applicability. This can over-refuse an unrelated model when a newly observed scoped label is exhausted; it cannot hide that exhaustion as unknown/zero. Add an explicit association to distinguish such a label. An unavailable/unreported numeric value remains `None`/unknown, never confirmed zero. Expired windows no longer count; the existing latest-observation, CLI TTL/negative-cache and reset-time algorithms are unchanged. Paid overage windows remain ignored as before.
- These associations grant no eligibility: enabled/deny/providers/automatic-membership, consent and observed-catalog restrictions still apply. Withdrawn or renamed models do not inherit a scoped rule. The route menu exposes effective rules and `unmapped: conservative_shared` in `quota_window_policy`; automatic selection and failure-reset recording use the same winning first-existing project → environment → user settings. Raw status windows remain engine evidence, not a fabricated model-specific quota.
- Invalid mappings produce `invalid_routing_overrides` in runtime reads and `invalid_quota_windows` with a rule field in configuration updates, without a write or engine execution. An already reported engine refusal is still recorded with the existing bounded default expiry if invalid settings prevent resolving its reset.

**Editing and migration:** Routing → Quota window associations provides typed exact-label, shared/exact-model and model-list controls. Unknown/unrepresentable rule data makes the form read-only; it is not dropped on save. Revision conflicts require reload and explicit resubmission. CLI access remains independent of Paseo screen reachability: send configuration API v1 routing updates or use the explicit candidate migration script. For a file without mappings, copy its complete current settings to an owner candidate, add reviewed associations, preview, then apply against the preview's `expected_revision` and `candidate_sha256` with `--i-consent`. Existing decisions are preserved; the migration refuses changes to an existing association list, which instead requires a deliberate configuration update. Example preview:

The former candidate-migration helper (`cortex_model_policy_migrate`) was removed on 2026-11-16 without replacement; apply a candidate through the Settings API under the revision rules above.

Saved profiles capture this field on explicit save/create and restore it on activation; older snapshots are not silently rewritten or filled from working settings/template data. Migrate a saved snapshot from its own candidate with `--profile`, or save the intended current working settings explicitly. Deleting a selected profile still only unlinks it, without resetting materialized settings.

The public install-only asset retains the previous route policy with the explicit Haiku 5.5/Luna-parity addition above, disabled OpenCode, and explicit canonical scoped-label associations preserving the former quota predicate for the configured identities. No runtime template overlay exists. Unknown/new provider labels remain conservative until configured; this is not evidence those labels are currently emitted or that production has adopted the candidate. Production application requires separate reviewed/verified adoption.

### Explicit user model priority and usage switching

For each node it covers, an explicit model request confirmed from recorded user
messages or answers takes priority over automatic role tiers, pins, preferred
models (including conditional preferences), automatic membership, usage ranking
and retry diversification. The coordinator carries the exact engine/model on
covered nodes, including retries/fixes; other nodes route normally. This applies
to light, standard, deep and flagship models. Hard owner deny/disabled/provider,
task route/profile/recipient authorization, missing configured/observed identity
or supported effort, engine unavailability and exhausted quota still refuse
visibly with no silent substitution. No model-authored text establishes consent.
A model requirement in the project root's own `AGENTS.md` or `CLAUDE.md` has the
same priority for the role purpose it names, as the file stood when the task
started (an edit made during the task, including a worker's, grants nothing for
that task); a newer user message outranks it. See
[project instruction requirements](features/routing-and-fallback/index.md#project-instruction-model-requirements).

Plan usage is only the last tie-breaker within a tier (rank version 15): among
candidates equal on owner preferences, honoured hints, the 95% hot band
(HOT_PERCENT in routing.auto_route), retry diversification and engine affinity,
the lower known usage wins. It never overrides an eligible route the coordinator
named; such a route is replaced only when its known usage is hot while another
candidate is not. While a `preferred_for` choice for the work kind is honoured,
usage compares only those preferred models with each other; when the tier has
preferred models but none is honoured (hot and meaningfully worse, exhausted or
unavailable), usage compares nothing and the replacement follows the quota band,
retry diversification, engine affinity and engine order.
There is no configuration field for this. Unknown usage is
unavailable, not hot. Audit usage status reports active or unavailable, with
the hot threshold and the observed candidate percentages. Exhaustion at 100%
is still a hard exclusion. Usage ranking never replaces a confirmed user
model request. See [routing priority](features/routing-and-fallback/index.md#user-requested-flagship-route).
