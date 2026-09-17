# codex-acp adapter test fixtures (T013)

`install-record.template.json` is a **shape-only template** for the JSON a
real operator would pass to `scripts/cortex_codex_probe.py --install-record`
(see `engine.codex_install.CodexAcpInstall`). It is never a real pinned
install: every hash/version/path field is a placeholder. `tests/
test_engine_codex_install.py`, `tests/test_engine_codex_launch.py`, and
`tests/test_engine_codex_probe.py` build their own synthetic, owned
`CodexAcpInstall` fixtures fresh under `tmp_path` at test time (real files,
real freshly-computed hashes) rather than depending on this static
template's placeholder hashes matching anything real — this file exists
only to document the exact JSON shape `cortex_codex_probe.py --install-record`
expects, and is never loaded by the automated test suite.

The observed pinned `configOptions` shape for the qualified
`@agentclientprotocol/codex-acp` 1.12.0 release used throughout this
task's tests lives at `tests/fixtures/catalog/codex_acp_config_options.json`
(shared with the T007/T008 catalog/configuration tests; see that file's own
`_fixture_note` for the `dist/index.js` line citations it is derived from) —
it is intentionally not duplicated here.
