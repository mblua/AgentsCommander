# Release preparation plan: v0.31.0

Status: `REVIEW_REQUIRED`. This generated bundle is not approval to implement, tag, publish, release, or deploy.

Repository: `mblua/AgentsCommander`  
Release issue: https://github.com/mblua/AgentsCommander/issues/1893  
Candidate: `0.31.0` / `v0.31.0`  
Planning base: `f5c4720f46a164cb03a01b3d2283d73653133e15`  
Predecessor: `v0.30.5`

## 1. Exact review identity

The reviewed object is the complete generated bundle. Certify the exact `SHA256SUMS` bytes and separately record its SHA-256. No artifact may be regenerated, reformatted, or copied through a newline-changing tool after certification.

The annotated-tag message is the exact bytes of `release-authority-v1.txt`. Its `review-set-sha256` binds the plan, evidence manifest, changelog input, Release body, and both asset ledgers without a circular self-hash. The candidate tree must contain every bundle file at `docs/releases/v0.31.0/` before tagging.

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

- `docs/releases/v0.31.0/CHANGELOG.release.md`
- `docs/releases/v0.31.0/candidate-assets.v1.json`
- `docs/releases/v0.31.0/input-manifest.v1.json`
- `docs/releases/v0.31.0/predecessor-assets.v1.json`
- `docs/releases/v0.31.0/release-authority-v1.txt`
- `docs/releases/v0.31.0/release-body.md`
- `docs/releases/v0.31.0/release-plan.md`
- `docs/releases/v0.31.0/SHA256SUMS`

The final canonical release-hardening plan is not generated or sealed by this bundle. Only its deterministic repository path is present in the hardening allowlist. It remains outside `docs/releases/`, `SHA256SUMS`, `review-set-v1`, and the bundle archive. After exact-bundle approval, the architect authors that separate plan with `READY_FOR_IMPLEMENTATION`; the plan may reference the approved bundle (`P -> B`), but this bundle never hashes or incorporates the future plan.

## 2. Frozen read-only facts

- Git remote main and GitHub API agree at `f5c4720f46a164cb03a01b3d2283d73653133e15`.
- Ordered planning-base parents: [54d0ac3e54420ef0a90a353cf3d6edf64dba408e, 9097700655123b6b0111dc7bd0388ee7bbff85f3].
- Base `.github/workflows/release.yml` blob: `af2a50413e83837a4ab32e0f0e0059172eb47cee`; content SHA-256: `bda546d06d78935e9fa0f06ba91714f9b76d0c48c70526d2ff1111f12ae62c03`.
- Predecessor annotated object: `69e00f8f2c8cd3c72ef611d310a0487247616db1`; peeled commit: `535ceb1502049b813154ab32c6bb2197300c6f33`.
- Predecessor immutable GitHub Release id: `383676971`.
- npm latest: `0.30.5`; candidate tag, Release, and npm version are absent at both discovery snapshots.
- Ruleset/reviewer authority: rules require approval; `mblua` has the documented admin exception only for unavailable self-review after every check passes.
- Tag immutability/binding policy: the repository lacks a selected tag-protection guarantee, so every job/rerun and final verifier fail on any object/peeled movement.
- Release asset upload design: direct uploader jobs receive job-local contents: write and no id-token.
- Frozen GitHub CLI: `gh 2.100.0`, archive `gh_2.100.0_linux_amd64.tar.gz`, SHA-256 `e4d4bb4498e8d007abe545b6568926793ace1b6447da598294a610018cb164be`.

Resolved action pins:

- `actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09` (discovered from `fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09`)
- `actions/setup-node@a0853c24544627f65ddf259abe73b1d18a591444` (discovered from `a0853c24544627f65ddf259abe73b1d18a591444`)
- `actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02` (discovered from `ea165f8d65b6e75b540449e92b4886f43607fa02`)
- `dtolnay/rust-toolchain@4360b52568e2003a75bf9bc1d59f33a8e3fc893c` (discovered from `4360b52568e2003a75bf9bc1d59f33a8e3fc893c`)
- `swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6` (discovered from `6323deb102c322ba6fcbdcafc7e3dddab59af2b6`)
- `tauri-apps/tauri-action@84b9d35b5fc46c1e45415bdb6144030364f7ebc5` (discovered from `84b9d35b5fc46c1e45415bdb6144030364f7ebc5`)

Current version surfaces to advance with the repository's version tool:

- `package.json` `/version`: `0.30.5` (blob `7b0f57bd86aaa456cfba8482365abe19d643cc0a`)
- `package-lock.json` `/version`: `0.30.5` (blob `554842c7d623342416825558df616434c7b88631`)
- `package-lock.json` `/packages//version`: `0.30.5` (blob `554842c7d623342416825558df616434c7b88631`)
- `npm/package.json` `/version`: `0.30.5` (blob `46e2d3f9a82784f9616aa483b7362b4fcbacfab5`)
- `npm/install.js` `const VERSION`: `0.30.5` (blob `3e0cea91d890745a29f6c0fa4719a96f4c1f4808`)
- `src-tauri/Cargo.toml` `[package].version`: `0.30.5` (blob `6ccf7e57fe010085ed2fd087c801d81b38d03af2`)
- `Cargo.lock` `agentscommander-new.version`: `0.30.5` (blob `a9930f49f5a0bae8db63d8194e2470daba8260db`)
- `src-tauri/tauri.conf.json` `/version`: `0.30.5` (blob `7cf8ad920c8019e253f5433f22b2a41e80aa7223`)

Approved scope:

- fix: fix(api): close the available_loopback_port() TOCTOU race (refs #1768) ([#1768](https://github.com/mblua/AgentsCommander/issues/1768), [PR #1870](https://github.com/mblua/AgentsCommander/pull/1870))
- fix: fix(testability): name the testable_gui_active mutex holder and close panic-path leaks (refs #1773) ([#1773](https://github.com/mblua/AgentsCommander/issues/1773), [PR #1890](https://github.com/mblua/AgentsCommander/pull/1890))
- feature: feat(#1795): seed six shared filesystem locations in the Golden Rule block ([#1795](https://github.com/mblua/AgentsCommander/issues/1795), [PR #1845](https://github.com/mblua/AgentsCommander/pull/1845))
- fix: fix(test): mock PtyAPI so npm test cannot exit non-zero with every test passing (refs #1797) ([#1797](https://github.com/mblua/AgentsCommander/issues/1797), [PR #1846](https://github.com/mblua/AgentsCommander/pull/1846))
- feature: feat(#1801): add the restart-resume settings contract and the persisted-working predicate ([#1801](https://github.com/mblua/AgentsCommander/issues/1801), [PR #1848](https://github.com/mblua/AgentsCommander/pull/1848))
- feature: feat(#1802): phase 1b - wake policy, target collection and the restart auto-resume pass ([#1802](https://github.com/mblua/AgentsCommander/issues/1802), [PR #1872](https://github.com/mblua/AgentsCommander/pull/1872))
- feature: feat(settings): add the On app restart section to Settings > General (refs #1803) ([#1803](https://github.com/mblua/AgentsCommander/issues/1803), [PR #1878](https://github.com/mblua/AgentsCommander/pull/1878))
- docs: docs(settings): document the On app restart settings (refs #1804) ([#1804](https://github.com/mblua/AgentsCommander/issues/1804), [PR #1880](https://github.com/mblua/AgentsCommander/pull/1880))
- maintenance: test(list-peers): pin D3 against a status-predicate selector, and fix an inverted fixture comment (refs #1822) ([#1822](https://github.com/mblua/AgentsCommander/issues/1822), [PR #1849](https://github.com/mblua/AgentsCommander/pull/1849))
- maintenance: ci(1826): harden publish-npm verification; add --version to the CLI root ([#1826](https://github.com/mblua/AgentsCommander/issues/1826), [PR #1827](https://github.com/mblua/AgentsCommander/pull/1827))
- feature: feat(sidebar): persist and tokenise the selected-row rail (refs #1796) ([#1828](https://github.com/mblua/AgentsCommander/issues/1828), [PR #1836](https://github.com/mblua/AgentsCommander/pull/1836))
- feature: feat(#1829): mirror the two rail fields in the TypeScript AppSettings contract ([#1829](https://github.com/mblua/AgentsCommander/issues/1829), [PR #1837](https://github.com/mblua/AgentsCommander/pull/1837))
- feature: feat(#1830): publish the selected-row rail settings to the DOM and edit them in the modal ([#1830](https://github.com/mblua/AgentsCommander/issues/1830), [PR #1838](https://github.com/mblua/AgentsCommander/pull/1838))
- docs: docs(npm): describe published 0.30.5 resolver on the package page (refs #1834) ([#1834](https://github.com/mblua/AgentsCommander/issues/1834), [PR #1843](https://github.com/mblua/AgentsCommander/pull/1843))
- feature: feat(#1844): default the selected row bar color to #630707 ([#1844](https://github.com/mblua/AgentsCommander/issues/1844), [PR #1847](https://github.com/mblua/AgentsCommander/pull/1847))
- fix: fix(1856): reconcile session communication from the polled listing ([#1856](https://github.com/mblua/AgentsCommander/issues/1856), [PR #1864](https://github.com/mblua/AgentsCommander/pull/1864))
- fix: fix(1857): derived aggregated blocked-menu toast, taskbar attention, pinned toasts, exit race ([#1857](https://github.com/mblua/AgentsCommander/issues/1857), [PR #1866](https://github.com/mblua/AgentsCommander/pull/1866))
- fix: fix(1858): give the blocked menu its own glyph and colour, and style .toast-item__action ([#1858](https://github.com/mblua/AgentsCommander/issues/1858), [PR #1869](https://github.com/mblua/AgentsCommander/pull/1869))
- fix: fix(1859): roll the blocked-menu state up through every level that can hide a row ([#1859](https://github.com/mblua/AgentsCommander/issues/1859), [PR #1874](https://github.com/mblua/AgentsCommander/pull/1874))
- feature: feat(catalog): add Muse Code to the embedded coding-agents catalog (#1860) ([#1860](https://github.com/mblua/AgentsCommander/issues/1860), [PR #1888](https://github.com/mblua/AgentsCommander/pull/1888))
- maintenance: refactor(sidebar): one catalogue component for session-row context menus; Root Agent menu at parity (#1871) ([#1871](https://github.com/mblua/AgentsCommander/issues/1871), [PR #1886](https://github.com/mblua/AgentsCommander/pull/1886))
- docs: docs: document codebase-memory-mcp for Claude Code and drop the MCP stance (#1882) ([#1882](https://github.com/mblua/AgentsCommander/issues/1882), [PR #1885](https://github.com/mblua/AgentsCommander/pull/1885))
- docs: docs(1893): fill Unreleased for 0.31.0 ([#1893](https://github.com/mblua/AgentsCommander/issues/1893), [PR #1894](https://github.com/mblua/AgentsCommander/pull/1894))

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

Before implementation, independently query remote main through raw Git and the GitHub ref API. Both must equal `f5c4720f46a164cb03a01b3d2283d73653133e15`; the GitHub commit API must return the ordered parent list [54d0ac3e54420ef0a90a353cf3d6edf64dba408e, 9097700655123b6b0111dc7bd0388ee7bbff85f3]; the contents API must return workflow blob `af2a50413e83837a4ab32e0f0e0059172eb47cee`. Any mismatch is `FROZEN_INPUT_CHANGED`: discard the bundle and run the generator again from a new config/output path.

This gate is not reused after required merges. Later gates bind `FINAL_CANDIDATE_MAIN` instead.

## 5. Hardening PR contract

Branch from the exact planning base. The exact changed-path allowlist is:

```text
.github/workflows/release.yml
docs/releases/v0.31.0/CHANGELOG.release.md
docs/releases/v0.31.0/candidate-assets.v1.json
docs/releases/v0.31.0/input-manifest.v1.json
docs/releases/v0.31.0/predecessor-assets.v1.json
docs/releases/v0.31.0/release-authority-v1.txt
docs/releases/v0.31.0/release-body.md
docs/releases/v0.31.0/release-plan.md
docs/releases/v0.31.0/SHA256SUMS
plans/1893-v0310-release-hardening.md
```

The allowlist is closed: no wildcard and no other changed path is permitted. Its final entry is the canonical plan path derived solely from the release issue number and candidate version; it is not a bundle artifact. The bundle files must be copied byte-for-byte from the reviewed output and verified against `SHA256SUMS` on the PR head. The workflow implementation must be generic for every normal `vX.Y.Z`; candidate-specific values live only in `docs/releases/$TAG/`.

For the selected merge method, record `HARDENING_HEAD` only after exact-head review. After protected merge, require a two-parent merge commit and assert the ordered parent vector is exactly:

```text
[f5c4720f46a164cb03a01b3d2283d73653133e15, HARDENING_HEAD]
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

Jobs that run GitHub attestation verification receive `attestations: read` and explicit `GH_TOKEN: ${{ github.token }}`. Before either attestation path, download only `gh_2.100.0_linux_amd64.tar.gz`, verify SHA-256 `e4d4bb4498e8d007abe545b6568926793ace1b6447da598294a610018cb164be`, install from that verified archive, and require `gh version 2.100.0` before use.

Each macOS architecture is built once. The raw binary, app archive, and installer for that architecture must derive from that one build and one recorded digest manifest; no second macOS build may generate a competing asset.

Derive the immediate predecessor dynamically by strict normal-SemVer comparison, then require its exact annotated object, peeled commit, immutable public Release, and asset ledger. Do not use mutable `latest` alone as predecessor authority.

### Changelog and Release body

Replace the exact current `Unreleased` body once: remove it from `Unreleased` and insert it under `## 0.31.0`. The expected complete result is `CHANGELOG.release.md`; use `release-body.md` verbatim for the GitHub Release. Duplicate or residual copies fail.

Every version-side scope/changelog declaration below is mandatory; a missing scope issue, summary substring, changelog substring, or whole-word alternative fails closed:

- Scope issue #1802: summary requires every case-insensitive substring ["auto-resume","wake policy"]; changelog requires every case-insensitive substring ["on app restart","restart prompts"] and at least one whole-word match from each alternative group [["resume","resumes"]].
- Scope issue #1857: summary requires every case-insensitive substring ["aggregated","toast"]; changelog requires every case-insensitive substring ["aggregated toast","taskbar attention"] and at least one whole-word match from each alternative group [["toast","toasts"]].
- Scope issue #1860: summary requires every case-insensitive substring ["muse code","catalog"]; changelog requires every case-insensitive substring ["muse code","beta preset"] and at least one whole-word match from each alternative group [["muse"]].

### Package and asset gates

Run the repository version tool for `0.31.0`; do not hand-edit version surfaces. Require all eight parsed surfaces to equal the candidate on the version PR head. Run `npm pack --dry-run`, create the tarball, inspect it, and smoke-install from that exact tarball before tagging.

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

After explicit authorization, create one annotated tag at `FINAL_CANDIDATE_MAIN` using the exact `docs/releases/v0.31.0/release-authority-v1.txt` bytes. Before push, require the local tag object type to be `tag`, its payload to match byte-for-byte, and its peeled commit to equal `FINAL_CANDIDATE_MAIN`.

After push, resolve the two remote records and execute exact assertions against the local annotated object and peeled commit. Do not merely display them. A missing record, extra record, changed object, or changed peeled commit stops all later claims.

The tag workflow then applies the every-job bootstrap, reuses only the exact draft for the exact tag on rerun, uploads/verifies the exact ledger, makes the Release immutable and public before npm, publishes npm once with provenance, and performs a clean install from the registry.

## 9. Recovery and completion

- Before GitHub publication, a failed run may resume only with the exact same annotated-object/peeled pair, payload, final candidate, draft id, and asset digests.
- After an immutable public Release exists, never delete, replace, or recreate it. If npm is absent, resume only the npm job from the exact verified Release and package tarball.
- If npm already contains the version, require exact repository/tag/provenance/tarball identity; otherwise stop for a new version.
- Never use `--clobber` to hide a digest mismatch. Idempotent overwrite is allowed only when the existing asset already belongs to the same guarded draft and the replacement digest is the exact expected digest.

Completion requires all 17 candidate assets and checksums, a public immutable GitHub Release, verified attestations, npm dist-tag/version/provenance/signature, clean npm install, and the destination executable reporting `0.31.0`. WG23 independently repeats GitHub/npm/executable verification before declaring the release complete.

## 10. Required negative suite

The workflow and preparation generator must reject: event SHA equated to tag object; missing candidate-tree evidence; a missing, changed, wildcarded, duplicated, or extra canonical-plan allowlist route; canonical-plan entry into `review-set-v1`, `SHA256SUMS`, or the bundle archive; a caller-supplied canonical-plan config key; reordered/duplicated execution; stale planning-base final gate; ancestry-only topology; absent/moved/lightweight/duplicate tag records; post-tag absence semantics; insufficient uploader permission or excess OIDC; duplicated/weakened changelog; mutable/mismatched GitHub CLI; missing npm registry/version/token gates; unsatisfied reviewer authority; malformed manifests/payloads; changed main; candidate collisions; ambiguous GitHub/npm errors; duplicate/missing/extra assets or facts; path traversal; output escape; caller-controlled CLI fixture injection; credential material in any input/evidence/error/artifact; and wrong `review-set-v1` order, byte, separator, terminator, or length-prefix semantics.
