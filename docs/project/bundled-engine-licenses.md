# Bundled engine licence inventory

The Workspace Debian packages include Node 24.14.1 and the npm distribution
installed by `npm ci --omit=dev --ignore-scripts` from
[`packaging/engines/package-lock.json`](../../packaging/engines/package-lock.json).
Every registry tarball has a locked version, URL and integrity hash. Node's
Linux x64/arm64 archive hashes are committed in
[`node-checksums.json`](../../packaging/engines/node-checksums.json).
OpenCode is not included. System libraries remain apt dependencies, not vendored files.

## Public distribution: owner risk decision (2026-11-06)

The owner reviewed Anthropic's published terms and decided to publish the bundled
Claude components in public GitHub releases, keeping native-account login unchanged
and accepting the risk themselves. This replaces the pending owner review gate.
It is an owner risk decision, not permission from Anthropic or a legal conclusion.

Anthropic's [legal and compliance guidance](https://code.claude.com/docs/en/legal-and-compliance)
allows preinstallation under its Commercial Terms subject to conditions: keep the
binary unmodified and all built-in authentication available; users authenticate
and pay for their own usage, without the distributor intermediating it.
That page also prohibits offering third-party claude.ai login and recommends API
keys for integrations. It does not prevent users signing into the unmodified binary.
The [Agent SDK overview](https://code.claude.com/docs/en/agent-sdk/overview)
requires prior approval for third-party claude.ai login and directs developers to
API-key authentication. Whether the unmodified-binary exception covers Cortex's
native-account flow through the Agent SDK remains unresolved.

[Consumer Terms, section 3](https://www.anthropic.com/legal/consumer-terms)
restrict automated access except via an API key or explicit permission. No
Anthropic approval for Cortex is recorded here. SDK/native npm licence declarations
are not open-source grants; the separate ACP adapter's Apache-2.0 licence does not
resolve them. Preserve their original licence files as well as the notices below.
The SDK branding guidance also disallows “Claude Code” as a product or feature label.

The Apache-2.0, MIT, BSD, ISC and Unlicense declarations below still require their
applicable notices. Original package licence/notice files remain in the installed
`engines/node_modules` tree; Node's full `engines/node/LICENSE` includes its
third-party notices. Preserve those texts with the distribution. Node reports
its embedded component versions through `process.versions`; this inventory
lists distribution units rather than claiming to inventory statically linked
upstream source components independently.

## Distribution units

Node.js **24.14.1** — MIT plus the third-party licences in its distribution LICENSE.
The table below records the **124 installed npm distribution units** observed in the
Linux amd64 build, including nested duplicate versions and both installed Claude
glibc/musl native variants. An arm64 build selects the corresponding
`codex-linux-arm64` (`0.164.0-linux-arm64`) and
`claude-agent-sdk-linux-arm64[-musl]` (`0.3.310`) with the same respective
Apache-2.0 and Anthropic licence declarations; arm64 execution is not qualified.
Non-Linux optional packages in the lock are not shipped on Linux.

| Name | Version | Declared licence |
| --- | --- | --- |
| @agentclientprotocol/claude-agent-acp | 0.97.0 | Apache-2.0 |
| @agentclientprotocol/codex-acp | 2.3.0 | Apache-2.0 |
| @agentclientprotocol/sdk | 2.0.2 | Apache-2.0 |
| @anthropic-ai/claude-agent-sdk | 0.3.310 | SEE LICENSE IN README.md |
| @anthropic-ai/claude-agent-sdk-linux-x64 | 0.3.310 | SEE LICENSE IN LICENSE.md |
| @anthropic-ai/sdk | 1.6.0 | Apache-2.0 |
| @babel/runtime | 7.29.13 | MIT |
| @hono/node-server | 2.1.6 | MIT |
| @modelcontextprotocol/sdk | 1.32.2 | MIT |
| @openai/codex | 0.164.0 | Apache-2.0 |
| @openai/codex-linux-x64 | 0.164.0-linux-x64 | Apache-2.0 |
| @opentelemetry/api | 1.10.0 | Apache-2.0 |
| @stablelib/base64 | 1.0.1 | MIT |
| accepts | 2.0.0 | MIT |
| ajv | 8.20.0 | MIT |
| ajv-formats | 3.0.1 | MIT |
| body-parser | 2.4.0 | MIT |
| content-type | 2.1.0 | MIT |
| bundle-name | 4.1.1 | MIT |
| bytes | 3.1.2 | MIT |
| call-bind-apply-helpers | 1.0.2 | MIT |
| call-bound | 1.0.4 | MIT |
| content-disposition | 2.0.1 | MIT |
| content-type | 1.0.5 | MIT |
| cookie | 0.7.2 | MIT |
| cookie-signature | 1.2.2 | MIT |
| cors | 2.8.6 | MIT |
| cross-spawn | 7.0.6 | MIT |
| debug | 4.4.3 | MIT |
| default-browser | 5.5.1 | MIT |
| default-browser-id | 5.0.1 | MIT |
| define-lazy-prop | 3.0.0 | MIT |
| depd | 2.0.0 | MIT |
| diff | 9.0.0 | BSD-3-Clause |
| dunder-proto | 1.0.1 | MIT |
| ee-first | 1.1.1 | MIT |
| encodeurl | 2.0.1 | MIT |
| es-define-property | 1.0.1 | MIT |
| es-errors | 1.3.0 | MIT |
| es-object-atoms | 1.1.2 | MIT |
| escape-html | 1.0.3 | MIT |
| etag | 1.8.1 | MIT |
| eventsource | 3.0.7 | MIT |
| eventsource-parser | 3.1.1 | MIT |
| express | 5.3.0 | MIT |
| express-rate-limit | 8.7.3 | MIT |
| content-type | 2.1.0 | MIT |
| fast-deep-equal | 3.1.3 | MIT |
| fast-sha256 | 1.3.0 | Unlicense |
| fast-uri | 3.1.9 | BSD-3-Clause |
| finalhandler | 2.1.1 | MIT |
| forwarded | 0.2.0 | MIT |
| fresh | 2.0.0 | MIT |
| function-bind | 1.1.2 | MIT |
| get-intrinsic | 1.3.0 | MIT |
| get-proto | 1.0.1 | MIT |
| gopd | 1.2.0 | MIT |
| has-symbols | 1.1.0 | MIT |
| hasown | 2.0.4 | MIT |
| hono | 4.13.16 | MIT |
| http-errors | 2.0.1 | MIT |
| iconv-lite | 0.7.3 | MIT |
| inherits | 2.0.4 | ISC |
| ip-address | 10.7.6 | MIT |
| ipaddr.js | 1.9.1 | MIT |
| is-docker | 3.0.0 | MIT |
| is-in-ssh | 1.0.0 | MIT |
| is-inside-container | 1.0.0 | MIT |
| is-promise | 4.0.0 | MIT |
| is-wsl | 3.1.1 | MIT |
| isexe | 2.0.0 | ISC |
| jose | 6.2.15 | MIT |
| json-schema-to-ts | 3.1.1 | MIT |
| json-schema-traverse | 1.0.0 | MIT |
| json-schema-typed | 8.0.2 | BSD-2-Clause |
| math-intrinsics | 1.1.0 | MIT |
| media-typer | 1.1.1 | MIT |
| merge-descriptors | 2.0.0 | MIT |
| mime-db | 1.54.0 | MIT |
| mime-types | 3.0.2 | MIT |
| ms | 2.1.3 | MIT |
| negotiator | 1.1.0 | MIT |
| content-type | 2.1.0 | MIT |
| object-assign | 4.1.1 | MIT |
| object-inspect | 1.13.4 | MIT |
| on-finished | 2.4.2 | MIT |
| once | 1.4.0 | ISC |
| open | 11.0.4 | MIT |
| parseurl | 1.3.3 | MIT |
| path-key | 3.1.1 | MIT |
| path-to-regexp | 8.4.2 | MIT |
| pkce-challenge | 5.0.1 | MIT |
| powershell-utils | 0.2.1 | MIT |
| proxy-addr | 2.0.8 | MIT |
| qs | 6.17.1 | BSD-3-Clause |
| range-parser | 1.3.0 | MIT |
| raw-body | 3.0.2 | MIT |
| require-from-string | 2.0.2 | MIT |
| router | 2.2.0 | MIT |
| run-applescript | 7.1.0 | MIT |
| safer-buffer | 2.1.2 | MIT |
| send | 1.2.1 | MIT |
| serve-static | 2.2.1 | MIT |
| setprototypeof | 1.2.0 | ISC |
| shebang-command | 2.0.0 | MIT |
| shebang-regex | 3.0.0 | MIT |
| side-channel | 1.1.1 | MIT |
| side-channel-list | 1.0.1 | MIT |
| side-channel-map | 1.0.1 | MIT |
| side-channel-weakmap | 1.0.2 | MIT |
| standardwebhooks | 1.1.1 | MIT |
| statuses | 2.0.2 | MIT |
| toidentifier | 1.0.1 | MIT |
| ts-algebra | 2.0.0 | MIT |
| type-is | 2.1.0 | MIT |
| content-type | 2.1.0 | MIT |
| unpipe | 1.0.0 | MIT |
| vary | 1.1.2 | MIT |
| vscode-jsonrpc | 9.0.3 | MIT |
| which | 2.0.2 | ISC |
| wrappy | 1.0.2 | ISC |
| wsl-utils | 1.0.1 | MIT |
| zod | 4.6.7 | MIT |
| zod-to-json-schema | 3.25.2 | ISC |

The installed `engines/bundle-manifest.json` retains the architecture-specific
inventory and SHA-256 for every regular bundled file. Packaging rechecks every
file before making the archive. A complete dependency tree is not a security
audit or evidence of a paid engine workload.
