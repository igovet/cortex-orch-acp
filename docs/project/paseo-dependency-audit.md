# Paseo plugin dependency audit (2026-10-04)

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed.


This is a bounded disposition of the plugin lockfile, not a clean security bill
or an audit of the installed Paseo host.

## Update 2026-10-14

A later `npm audit` at `integrations/paseo` reported 21 entries: one critical
(`shell-quote`, command injection, range `>=1.8.4 <1.11.0`, transitive through
`react-native`), 15 high and 5 moderate. The non-breaking `npm audit fix` (without
`--force`) raised `shell-quote` from 1.10.0 to 1.12.0 in `package-lock.json`; `package.json`
was not changed. The critical entry is resolved. The typecheck and the
`test:activation` and `test:steer` scripts passed after that fix.

The remaining 15 high and 5 moderate entries are all transitive through the
`react-native` development dependency tree (`@jest/*`, `metro*`, `micromatch`/`braces`,
`js-yaml`/`argparse`, `sprintf-js`). The only offered fix is `react-native@0.87.1`, a
semver-major devDependency bump, which is out of scope for a non-breaking fix. These
entries are not dispositioned here beyond the notes above; their reachability is not
assessed, and the plugin remains a development dependency only.

`npm --prefix integrations/paseo audit --json` reports 15 high-severity package
entries, all propagated from one distinct advisory:
[GHSA-vfj7-8cjw-p6xm](https://github.com/advisories/GHSA-vfj7-8cjw-p6xm)
(CVE-2026-93687), uncontrolled recursion in `braces <=3.0.3`. The
[upstream issue](https://github.com/micromatch/braces/issues/70) describes
stack exhaustion with deeply nested glob patterns. At this inspection the
advisory lists no patched version, and npm's latest published `braces` is
3.0.3. `micromatch` latest 4.0.8 still depends on `braces ^3.0.3`.

## Locked paths and disposition

All paths converge on the single locked `braces@3.0.3` through
`micromatch@4.0.8`. From `react-native@0.81.5` they arrive via:

- `babel-jest@29.7.0 -> @jest/transform@29.7.0`, directly or through
  `jest-haste-map@29.7.0`;
- `jest-environment-node@29.7.0 -> @jest/fake-timers@29.7.0 ->
  jest-message-util@29.7.0` (also through `@jest/environment@29.7.0`);
- `@react-native/community-cli-plugin@0.81.5 -> metro@0.83.8 ->
  metro-file-map@0.83.8`, including Metro's config/transform-worker cycles.

The 15 reported package entries are `braces`, `micromatch`, `@jest/transform`,
`jest-haste-map`, `jest-message-util`, `@jest/fake-timers`, `@jest/environment`,
`babel-jest`, `jest-environment-node`, `metro-file-map`, `metro`, `metro-config`,
`metro-transform-worker`, `@react-native/community-cli-plugin`, `react-native`.
These are not 15 independently demonstrated exploitable defects.

No safe patched dependency/override is available for this advisory today.
Keep the manifest and lockfile unchanged rather than hiding the chain. The
suggested React Native 0.87.1 migration requires React ^19.2.3 (currently
19.1.0), changes the Hermes compiler contract, and is not a compatible small
fix. Latest `metro-file-map@0.87.1` still uses `micromatch ^4.0.4`, so that
migration is not established to remove the underlying advisory either.

All packages being `devDependencies` does **not** establish non-reachability:
SDK imports and compilation tooling can affect delivered code. The current
client-bundle test externalizes host React/React Native/SDK modules, compiles
with esbuild and verifies via metafile that Metro/Jest/braces/micromatch inputs
are absent from that generated client artifact. React Native also supplies the
Hermes compiler used by this test, so removing it merely to silence audit would
lose real compatibility coverage. This test is not proof about the installed
host's own bundle or every installation/build path. Do not feed untrusted glob
patterns to affected tooling; no exploitability waiver or runtime mitigation
is claimed here.

## Recheck and next decision

Run from the checkout root, retaining full outputs and exit statuses:

```bash
npm --prefix integrations/paseo ci
npm --prefix integrations/paseo audit --json
npm --prefix integrations/paseo explain braces --json
npm view braces version versions dist-tags --json
npm view micromatch@latest version dependencies --json
npm --prefix integrations/paseo run typecheck
npm --prefix integrations/paseo run test:client-bundle
```

A nonzero audit exit remains a failed security check even when installation
and tests pass. Network/registry failure is not zero findings. Do not use
`audit fix --force`, suppress entries, remove the lockfile or upgrade the
production host. Recheck for a published upstream recursion fix, then update
within compatible ranges and requalify. Eliminating the chain before a patch
would need a separately reviewed tooling/SDK separation or maintained fork;
that is a new design decision, not a narrow dependency bump. No new paid call,
daemon or Desktop run is part of this audit; previous live evidence applies
to its exact older artifact only.
