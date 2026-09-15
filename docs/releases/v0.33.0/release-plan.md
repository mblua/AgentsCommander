# Release preparation plan: v0.33.0

Status: `REVIEW_REQUIRED`. This generated bundle is not approval to implement, tag, publish, release, or deploy.

Repository: `mblua/AgentsCommander`  
Release issue: https://github.com/mblua/AgentsCommander/issues/2033  
Candidate: `0.33.0` / `v0.33.0`  
Planning base: `fcf96beba35189c64215fc711c40087a4de68d2d`  
Predecessor: `v0.32.0`

## 1. Exact review identity

The reviewed object is the complete generated bundle. Certify the exact `SHA256SUMS` bytes and separately record its SHA-256. No artifact may be regenerated, reformatted, or copied through a newline-changing tool after certification.

The annotated-tag message is the exact bytes of `release-authority-v1.txt`. Its `review-set-sha256` binds the plan, evidence manifest, changelog input, Release body, and both asset ledgers without a circular self-hash. The candidate tree must contain every bundle file at `docs/releases/v0.33.0/` before tagging.

### review-set-v1 byte construction

The authoritative machine-readable recipe is `input-manifest.v1.json.contracts.reviewSet` with schema `prepare-agentscommander-release/review-set/v1`. Recompute it as follows; do not hardcode the candidate digest:

1. Read these basenames in this exact order, with no directory prefix:

```text
CHANGELOG.release.md
candidate-assets.v1.json
input-manifest.v1.json
predecessor-assets.v1.json
release-body.md
release-plan.md
```

2. Each file input is its exact emitted UTF-8 byte sequence. Perform no parsing, reserialization, Unicode normalization, or line-ending conversion. Text permits LF byte `0a` only, forbids a BOM and CR, and ends in exactly one LF. Generated JSON is already canonical: keys recursively sorted by ECMAScript UTF-16 string order, `JSON.stringify(value, null, 2)`, then one LF.
3. For each file, compute SHA-256 over those exact bytes and encode the digest as 64 lowercase hexadecimal ASCII bytes.
4. Serialize one record as `digest || 0x20 0x20 || UTF-8 basename || 0x0a`. There is no preamble, postamble, NUL, inter-record data, or length prefix. Concatenate records directly in the stated order; the last record retains its LF.
5. SHA-256 the complete concatenated record bytes and encode the result as 64 lowercase hexadecimal characters. That value must equal the sole `review-set-sha256` field in `release-authority-v1.txt`.

Positive conformance vector:

- `alpha.txt`: content base64 `YWxwaGEK`; SHA-256 `b6a98d9ce9a2d9149288fa3df42d377c3e42737afdcdaf714e33c0a100b51060`
- `beta.json`: content base64 `e30K`; SHA-256 `ca3d163bab055381827226140568f3bef7eaac187cebd76878e0b63e9e442356`

- exact order: `alpha.txt -> beta.json`
- serialized record bytes, base64: `YjZhOThkOWNlOWEyZDkxNDkyODhmYTNkZjQyZDM3N2MzZTQyNzM3YWZkY2RhZjcxNGUzM2MwYTEwMGI1MTA2MCAgYWxwaGEudHh0CmNhM2QxNjNiYWIwNTUzODE4MjcyMjYxNDA1NjhmM2JlZjdlYWFjMTg3Y2ViZDc2ODc4ZTBiNjNlOWU0NDIzNTYgIGJldGEuanNvbgo=`
- expected review-set SHA-256: `9d3945ec3d10e79abc60f28fd95e94cfb9d7f6b95ade412ea7bac4c138956ff8`

An implementation must first reproduce every vector byte and digest, then compute the candidate digest from candidate-tree files. A vector mismatch, different order, changed file byte, one-space separator, CRLF terminator, prefix, suffix, or normalization is fatal.

Generated evidence paths allowed in the hardening PR:

- `docs/releases/v0.33.0/CHANGELOG.release.md`
- `docs/releases/v0.33.0/candidate-assets.v1.json`
- `docs/releases/v0.33.0/input-manifest.v1.json`
- `docs/releases/v0.33.0/predecessor-assets.v1.json`
- `docs/releases/v0.33.0/release-authority-v1.txt`
- `docs/releases/v0.33.0/release-body.md`
- `docs/releases/v0.33.0/release-plan.md`
- `docs/releases/v0.33.0/SHA256SUMS`

The final canonical release-hardening plan is not generated or sealed by this bundle. Only its deterministic repository path is present in the hardening allowlist. It remains outside `docs/releases/`, `SHA256SUMS`, `review-set-v1`, and the bundle archive. After exact-bundle approval, the architect authors that separate plan with `READY_FOR_IMPLEMENTATION`; the plan may reference the approved bundle (`P -> B`), but this bundle never hashes or incorporates the future plan.

## 2. Frozen read-only facts

- Git remote main and GitHub API agree at `fcf96beba35189c64215fc711c40087a4de68d2d`.
- Ordered planning-base parents: [329ab94ed011a427b70c760093d5b542b4987d94, d26ea7dd5bdf3c6092faa041cafa237df6181afa].
- Base `.github/workflows/release.yml` blob: `8817fc60498807a0214d917fe42e5a389aef9ef1`; content SHA-256: `f61ba23479560e241bdff387971a9ec9651c590ec21f7ca4ecf540fbc3d65434`.
- Predecessor annotated object: `d5e8def6de77b47c828dc4ad5ddc9ac112d316d0`; peeled commit: `722a1752c27f9bf9f65e6c3ff0fed35847be0612`.
- Predecessor immutable GitHub Release id: `387717022`.
- npm latest: `0.32.0`; candidate tag, Release, and npm version are absent at both discovery snapshots.
- Ruleset/reviewer authority: rules require approval; `mblua` has the documented admin exception only for unavailable self-review after every check passes.
- Tag immutability/binding policy: the repository lacks a selected tag-protection guarantee, so every job/rerun and final verifier fail on any object/peeled movement.
- Release asset upload design: direct uploader jobs receive job-local contents: write and no id-token.
- Frozen GitHub CLI: `gh 2.101.0`, archive `gh_2.101.0_linux_amd64.tar.gz`, SHA-256 `9bca2d1c16825f109907a23307628a2f0698fbf99662b73a5cf0b020293072b8`.

Resolved action pins:

- `actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09` (discovered from `fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09`)
- `actions/setup-node@a0853c24544627f65ddf259abe73b1d18a591444` (discovered from `a0853c24544627f65ddf259abe73b1d18a591444`)
- `actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02` (discovered from `ea165f8d65b6e75b540449e92b4886f43607fa02`)
- `dtolnay/rust-toolchain@4360b52568e2003a75bf9bc1d59f33a8e3fc893c` (discovered from `4360b52568e2003a75bf9bc1d59f33a8e3fc893c`)
- `swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6` (discovered from `6323deb102c322ba6fcbdcafc7e3dddab59af2b6`)
- `tauri-apps/tauri-action@84b9d35b5fc46c1e45415bdb6144030364f7ebc5` (discovered from `84b9d35b5fc46c1e45415bdb6144030364f7ebc5`)

Current version surfaces to advance with the repository's version tool:

- `package.json` `/version`: `0.32.0` (blob `4b9ce86a973f1af1ed956cd4225426aed26fb4bf`)
- `package-lock.json` `/version`: `0.32.0` (blob `f9ab113973d9b30c54a3686f1e529c479e1ccbf6`)
- `package-lock.json` `/packages//version`: `0.32.0` (blob `f9ab113973d9b30c54a3686f1e529c479e1ccbf6`)
- `npm/package.json` `/version`: `0.32.0` (blob `776f7f12ac9690e44728a244336907c7ec545e9a`)
- `npm/install.js` `const VERSION`: `0.32.0` (blob `9000715ccef1efe0dd94759732276a134fc9c969`)
- `src-tauri/Cargo.toml` `[package].version`: `0.32.0` (blob `b48a0ed3215528e361f2f55edafc4d9854b2f3dd`)
- `Cargo.lock` `agentscommander.version`: `0.32.0` (blob `d65b6d29cfe26219da02e5dfecc85a0180a8c903`)
- `src-tauri/tauri.conf.json` `/version`: `0.32.0` (blob `7b12c059d071e80a28f3ddae8d2a28e8a4d9be19`)

Approved scope:

- maintenance: fix(#1466): make context-alert retry ordering test deterministic ([#1466](https://github.com/mblua/AgentsCommander/issues/1466), [PR #1993](https://github.com/mblua/AgentsCommander/pull/1993))
- fix: fix(#1935): refuse suffixed executables without a writable adjacent config dir ([#1935](https://github.com/mblua/AgentsCommander/issues/1935), [PR #2020](https://github.com/mblua/AgentsCommander/pull/2020))
- feature: feat(config): preserve replica selections and creation defaults (refs #1939) ([#1939](https://github.com/mblua/AgentsCommander/issues/1939), [PR #1959](https://github.com/mblua/AgentsCommander/pull/1959))
- feature: feat(1940): retain selection ownership through restart completion ([#1940](https://github.com/mblua/AgentsCommander/issues/1940), [PR #1990](https://github.com/mblua/AgentsCommander/pull/1990))
- feature: feat(1941): expose scoped selection locks and defaults ([#1941](https://github.com/mblua/AgentsCommander/issues/1941), [PR #1992](https://github.com/mblua/AgentsCommander/pull/1992))
- feature: feat(1942): add typed selection-lock client and completion handling ([#1942](https://github.com/mblua/AgentsCommander/issues/1942), [PR #1994](https://github.com/mblua/AgentsCommander/pull/1994))
- feature: feat(1943): add selection locks, KEEP badges and explicit defaults ([#1943](https://github.com/mblua/AgentsCommander/issues/1943), [PR #2004](https://github.com/mblua/AgentsCommander/pull/2004))
- docs: docs: highlight global npm installation (refs #1955) ([#1955](https://github.com/mblua/AgentsCommander/issues/1955), [PR #1956](https://github.com/mblua/AgentsCommander/pull/1956))
- feature: feat(1968): persist managed coding-agent catalog ([#1968](https://github.com/mblua/AgentsCommander/issues/1968), [PR #1991](https://github.com/mblua/AgentsCommander/pull/1991))
- docs: docs: document managed coding-agent catalog (refs #1969) ([#1969](https://github.com/mblua/AgentsCommander/issues/1969), [PR #2008](https://github.com/mblua/AgentsCommander/pull/2008))
- maintenance: test: make session NotFound retry fixture deterministic (#1984) ([#1984](https://github.com/mblua/AgentsCommander/issues/1984), [PR #2005](https://github.com/mblua/AgentsCommander/pull/2005))
- fix: fix(1988): install Linux runtime before npm release verification ([#1988](https://github.com/mblua/AgentsCommander/issues/1988), [PR #1989](https://github.com/mblua/AgentsCommander/pull/1989))
- maintenance: test: stabilize capped shutdown ownership fixture (#1996) ([#1996](https://github.com/mblua/AgentsCommander/issues/1996), [PR #2000](https://github.com/mblua/AgentsCommander/pull/2000))
- fix: fix: forward current Codex final answers to Telegram, refs #1997 ([#1997](https://github.com/mblua/AgentsCommander/issues/1997), [PR #2007](https://github.com/mblua/AgentsCommander/pull/2007))
- maintenance: fix: synchronize critical-key cleanup assertions (refs #1998) ([#1998](https://github.com/mblua/AgentsCommander/issues/1998), [PR #2006](https://github.com/mblua/AgentsCommander/pull/2006))
- fix: fix(coding-agents): submit Hermes/OpenCode/Grok and disable Muse (#1999) ([#1999](https://github.com/mblua/AgentsCommander/issues/1999), [PR #2003](https://github.com/mblua/AgentsCommander/pull/2003))
- fix: fix(#2010): keep the same locked pair as an unchanged selection write ([#2010](https://github.com/mblua/AgentsCommander/issues/2010), [PR #2012](https://github.com/mblua/AgentsCommander/pull/2012))
- feature: feat(#2014): apply prototype v4 to the Coding Agent modal ([#2014](https://github.com/mblua/AgentsCommander/issues/2014), [PR #2027](https://github.com/mblua/AgentsCommander/pull/2027))
- fix: fix(npm): resolve the macOS .app bundle executable in the launcher (refs #2016) ([#2016](https://github.com/mblua/AgentsCommander/issues/2016), [PR #2025](https://github.com/mblua/AgentsCommander/pull/2025))
- fix: fix(2018): drop legacy codingAgentProfiles migration so profile labels survive restart ([#2018](https://github.com/mblua/AgentsCommander/issues/2018), [PR #2026](https://github.com/mblua/AgentsCommander/pull/2026))
- fix: fix(#2021): seed the instance coding-agent catalog when no project is registered ([#2021](https://github.com/mblua/AgentsCommander/issues/2021), [PR #2022](https://github.com/mblua/AgentsCommander/pull/2022))

## 3. One mandatory order

Perform these stages exactly once and in this order:

1. Freeze the exact evidence and generated bundle.
2. Obtain WG33 cold-plan `PASS` on the exact `SHA256SUMS` identity.
3. Certify the exact bytes and record the `SHA256SUMS` SHA-256.
4. Obtain explicit human approval naming that exact hash.
5. Purge the planning context required by the implementation workflow.
6. Cold-implement the generic release hardening and exact evidence bundle; independently review the exact PR head; merge it through the protected branch.
7. Create the version/evidence PR from that exact hardening merge, move the approved Unreleased changelog bytes, run the repository version tool, independently review the exact PR head, and merge it through the protected branch.
8. Run final authority, topology, package, workflow, collision, and remote-main gates.
9. Create and push one annotated tag with the exact `release-authority-v1.txt` payload.
10. Verify the GitHub workflow, immutable public Release, npm package/provenance/install, and destination executable.

No implementation, PR, merge, tag, or publication occurs before steps 1-5. There is no second hardening or version merge later in the sequence.

## 4. Planning-base gate

Before implementation, independently query remote main through raw Git and the GitHub ref API. Both must equal `fcf96beba35189c64215fc711c40087a4de68d2d`; the GitHub commit API must return the ordered parent list [329ab94ed011a427b70c760093d5b542b4987d94, d26ea7dd5bdf3c6092faa041cafa237df6181afa]; the contents API must return workflow blob `8817fc60498807a0214d917fe42e5a389aef9ef1`. Any mismatch is `FROZEN_INPUT_CHANGED`: discard the bundle and run the generator again from a new config/output path.

This gate is not reused after required merges. Later gates bind `FINAL_CANDIDATE_MAIN` instead.

## 5. Hardening PR contract

Branch from the exact planning base. The exact changed-path allowlist is:

```text
.github/workflows/release.yml
docs/releases/v0.33.0/CHANGELOG.release.md
docs/releases/v0.33.0/candidate-assets.v1.json
docs/releases/v0.33.0/input-manifest.v1.json
docs/releases/v0.33.0/predecessor-assets.v1.json
docs/releases/v0.33.0/release-authority-v1.txt
docs/releases/v0.33.0/release-body.md
docs/releases/v0.33.0/release-plan.md
docs/releases/v0.33.0/SHA256SUMS
plans/2033-v0330-release-hardening.md
```

The allowlist is closed: no wildcard and no other changed path is permitted. Its final entry is the canonical plan path derived solely from the release issue number and candidate version; it is not a bundle artifact. The bundle files must be copied byte-for-byte from the reviewed output and verified against `SHA256SUMS` on the PR head. The workflow implementation must be generic for every normal `vX.Y.Z`; candidate-specific values live only in `docs/releases/$TAG/`.

For the selected merge method, record `HARDENING_HEAD` only after exact-head review. After protected merge, require a two-parent merge commit and assert the ordered parent vector is exactly:

```text
[fcf96beba35189c64215fc711c40087a4de68d2d, HARDENING_HEAD]
```

Set `HARDENING_MERGE_SHA` to that merge commit. A squash, rebase, reversed parent order, extra parent, or intervening main commit invalidates this bundle.

### Generic workflow bootstrap in every job

Every job checks out the pushed tag with full history before useful work. Its first executable step must implement assertions equivalent to:

```bash
set -euo pipefail
test "$GITHUB_REF_TYPE" = "tag"
test "$GITHUB_REF_NAME" = "${GITHUB_REF#refs/tags/}"
TAG="$GITHUB_REF_NAME"
[[ "$TAG" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]
HEAD_COMMIT="$(git rev-parse 'HEAD^{commit}')"
PEELED_COMMIT="$(git rev-parse "$TAG^{}")"
TAG_OBJECT="$(git rev-parse "$TAG^{tag}")"
test "$GITHUB_SHA" = "${{ github.sha }}"
test "$GITHUB_SHA" = "$HEAD_COMMIT"
test "$HEAD_COMMIT" = "$PEELED_COMMIT"
test "$TAG_OBJECT" != "$PEELED_COMMIT"
mapfile -t REMOTE_TAG < <(git ls-remote origin "refs/tags/$TAG" "refs/tags/$TAG^{}")
test "${#REMOTE_TAG[@]}" -eq 2
printf '%s\n' "${REMOTE_TAG[@]}" | grep -Fx "$TAG_OBJECT"$'\t'"refs/tags/$TAG"
printf '%s\n' "${REMOTE_TAG[@]}" | grep -Fx "$PEELED_COMMIT"$'\t'"refs/tags/$TAG^{}"
```

`GITHUB_SHA`, `${{ github.sha }}`, `HEAD`, and the peeled commit are one identity. The annotated object is a second identity. `guard` exports both; every later job compares its local and remote pair to those immutable outputs. Printing the pair without assertions is forbidden.

The strict `release-authority-v1` parser must reject reordered, missing, extra, duplicate, unknown, or malformed fields; recompute the review-set digest from candidate-tree files; and compare every base/predecessor/source/policy value with the manifest.

### Tag-state semantics

- Before tag creation only: candidate absence is required across remote Git, GitHub Releases, and npm.
- In the tag workflow and every rerun: absence is failure. Exactly one existing annotated-object record and one peeled record must equal the guard pair and canonical payload.
- A different object on the same peeled commit is a moved tag and fails.

No claim of “no bypass” or immutable ref is allowed. Bind the tag equivalently by exporting the exact annotated-object/peeled pair from guard, re-querying and asserting it before useful work in every job/rerun, and repeating it after workflow completion. Any movement invalidates the run and requires a new version if publication occurred.

### Release construction and permissions

Keep this order:

```text
guard -> platform producers -> checksums -> publish-github -> publish-npm
```

`publish-github` makes the Release public only after the exact 17-entry candidate ledger, SHA-256 file, downloadable required assets, and attestations verify. `publish-npm` runs only after the GitHub Release is public and immutable. Use string-valued `make_latest: 'true'`.

`build`, `checksums`, and `publish-github` each receive job-local `contents: write` because they directly upload or mutate Release assets/state; none receives `id-token`.

Only `publish-npm` receives `id-token: write`; it receives `contents: read`, uses `actions/setup-node` at an immutable commit with `node-version: 22` and `registry-url: https://registry.npmjs.org`, installs exact npm `11.6.2`, asserts both versions, and publishes with provenance. It must fail if `NODE_AUTH_TOKEN`, `NPM_TOKEN`, or the legacy package token is present. No other job receives OIDC.

Jobs that run GitHub attestation verification receive `attestations: read` and explicit `GH_TOKEN: ${{ github.token }}`. Before either attestation path, download only `gh_2.101.0_linux_amd64.tar.gz`, verify SHA-256 `9bca2d1c16825f109907a23307628a2f0698fbf99662b73a5cf0b020293072b8`, install from that verified archive, and require `gh version 2.101.0` before use.

Each macOS architecture is built once. The raw binary, app archive, and installer for that architecture must derive from that one build and one recorded digest manifest; no second macOS build may generate a competing asset.

Derive the immediate predecessor dynamically by strict normal-SemVer comparison, then require its exact annotated object, peeled commit, immutable public Release, and asset ledger. Do not use mutable `latest` alone as predecessor authority.

### Changelog and Release body

Replace the exact current `Unreleased` body once: remove it from `Unreleased` and insert it under `## 0.33.0`. The expected complete result is `CHANGELOG.release.md`; use `release-body.md` verbatim for the GitHub Release. Duplicate or residual copies fail.

Every version-side scope/changelog declaration below is mandatory; a missing scope issue, summary substring, changelog substring, or whole-word alternative fails closed:

- No release-specific scope/changelog semantic assertions are declared.

### Package and asset gates

Run the repository version tool for `0.33.0`; do not hand-edit version surfaces. Require all eight parsed surfaces to equal the candidate on the version PR head. Run `npm pack --dry-run`, create the tarball, inspect it, and smoke-install from that exact tarball before tagging.

Predecessor ledger: exactly 17 unique uploaded nonempty assets. Candidate ledger: exactly 17 unique names derived from predecessor version substitution plus exactly these declared candidate-only assets:

- No candidate-only assets are declared.

Any missing, extra, duplicate, zero-size, digest mismatch, producer mismatch, or name mismatch fails.

## 6. Version/evidence PR and exact topology

Branch from exactly `HARDENING_MERGE_SHA`. The exact changed-path allowlist is:

```text
CHANGELOG.md
package.json
package-lock.json
npm/package.json
npm/install.js
src-tauri/Cargo.toml
Cargo.lock
src-tauri/tauri.conf.json
```

The root `CHANGELOG.md` must equal generated `CHANGELOG.release.md`. All generated evidence already committed by the hardening PR remains byte-identical. Record `VERSION_HEAD` only after exact-head review and every required check/version-sync/package gate passes.

After protected merge, require a two-parent merge commit and assert the ordered parent vector is exactly:

```text
[HARDENING_MERGE_SHA, VERSION_HEAD]
```

Set `VERSION_MERGE_SHA` and `FINAL_CANDIDATE_MAIN` to that commit. Ancestry alone is insufficient.

The repository currently has no second eligible identity. After every required check and version-sync passes on the exact head, `mblua` may use the documented admin merge path solely to satisfy unavailable self-review. Admin authority must never override a failed, missing, pending, or stale check.

## 7. Final pre-tag gates

Immediately before tagging, all of these must pass against fresh remote/API state:

- remote main and GitHub default-branch ref both equal `FINAL_CANDIDATE_MAIN`, not the old planning base;
- exact hardening and version ordered parent vectors match section 5 and 6;
- the workflow and generated bundle exist at the final candidate with exact reviewed hashes;
- workflow parser/permissions/action/CLI/npm/attestation fixtures pass;
- every version/package/changelog/asset contract passes;
- candidate tag, Release, and npm version remain unambiguously absent;
- required status checks passed on each exact reviewed PR head;
- review authority matches the documented policy, with no unmodeled bypass.

Any failure spends no version: stop without creating a tag.

## 8. Annotated tag and executable post-push proof

After explicit authorization, create one annotated tag at `FINAL_CANDIDATE_MAIN` using the exact `docs/releases/v0.33.0/release-authority-v1.txt` bytes. Before push, require the local tag object type to be `tag`, its payload to match byte-for-byte, and its peeled commit to equal `FINAL_CANDIDATE_MAIN`.

After push, resolve the two remote records and execute exact assertions against the local annotated object and peeled commit. Do not merely display them. A missing record, extra record, changed object, or changed peeled commit stops all later claims.

The tag workflow then applies the every-job bootstrap, reuses only the exact draft for the exact tag on rerun, uploads/verifies the exact ledger, makes the Release immutable and public before npm, publishes npm once with provenance, and performs a clean install from the registry.

## 9. Recovery and completion

- Before GitHub publication, a failed run may resume only with the exact same annotated-object/peeled pair, payload, final candidate, draft id, and asset digests.
- After an immutable public Release exists, never delete, replace, or recreate it. If npm is absent, resume only the npm job from the exact verified Release and package tarball.
- If npm already contains the version, require exact repository/tag/provenance/tarball identity; otherwise stop for a new version.
- Never use `--clobber` to hide a digest mismatch. Idempotent overwrite is allowed only when the existing asset already belongs to the same guarded draft and the replacement digest is the exact expected digest.

Completion requires all 17 candidate assets and checksums, a public immutable GitHub Release, verified attestations, npm dist-tag/version/provenance/signature, clean npm install, and the destination executable reporting `0.33.0`. WG23 independently repeats GitHub/npm/executable verification before declaring the release complete.

## 10. Required negative suite

The workflow and preparation generator must reject: event SHA equated to tag object; missing candidate-tree evidence; a missing, changed, wildcarded, duplicated, or extra canonical-plan allowlist route; canonical-plan entry into `review-set-v1`, `SHA256SUMS`, or the bundle archive; a caller-supplied canonical-plan config key; reordered/duplicated execution; stale planning-base final gate; ancestry-only topology; absent/moved/lightweight/duplicate tag records; post-tag absence semantics; insufficient uploader permission or excess OIDC; duplicated/weakened changelog; mutable/mismatched GitHub CLI; missing npm registry/version/token gates; unsatisfied reviewer authority; malformed manifests/payloads; changed main; candidate collisions; ambiguous GitHub/npm errors; duplicate/missing/extra assets or facts; path traversal; output escape; caller-controlled CLI fixture injection; credential material in any input/evidence/error/artifact; and wrong `review-set-v1` order, byte, separator, terminator, or length-prefix semantics.
